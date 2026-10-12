//! Partial erasing: subtracting a circular region from an element's outline, rather than
//! deleting the whole element (that's just [`Scene::remove_element`], used directly by the
//! whole-object eraser tool). An element whose stroke passes through the eraser circle is
//! split into however many pieces survive outside it — a closed shape (rect/ellipse) that
//! loses an arc becomes an open polyline; a line erased in the middle becomes two lines; a
//! fully-circled element disappears entirely.
//!
//! The geometry works on a *flattened* (straight-line-segment) copy of the path, not the
//! original Bezier curves — exact circle/Bezier intersection isn't worth the complexity here,
//! and [`SHAPE_PATH_TOLERANCE`]-grade flattening is already how this project treats
//! shape-tool paths elsewhere. Remaining pieces are rebuilt as plain polylines (`MoveTo`/
//! `LineTo` chains); a once-closed shape's surviving arcs are never re-closed, matching that
//! they're no longer a closed shape.

use glam::DVec2;
use kurbo::{BezPath, Point};

use crate::geometry::flatten_to_points;
use crate::scene::{ElementKind, Scene, DEFAULT_STROKE_WIDTH};

/// Flattening tolerance (board-space) for turning an element's Bezier path into a polyline
/// before clipping it against the eraser circle. Not zoom-scaled, same reasoning as
/// `blueboard-app`'s `SHAPE_PATH_TOLERANCE`: this is a fidelity constant for the clip
/// geometry, not a screen-space rendering tolerance.
const ERASE_FLATTEN_TOLERANCE: f64 = 0.25;

/// Erases the part of every element under `center` (board-space) within `radius` of it.
/// Returns whether anything in the scene actually changed (so a caller that redraws every
/// frame regardless can skip work, though none currently bother to check).
pub fn erase_circle(scene: &mut Scene, center: DVec2, radius: f64) -> bool {
    let center = Point::new(center.x, center.y);
    let min = DVec2::new(center.x - radius, center.y - radius);
    let max = DVec2::new(center.x + radius, center.y + radius);

    // Collected up front: the loop below both reads and mutates `scene`, which an iterator
    // borrowed from `scene.elements_in_viewport` can't survive.
    let candidate_ids: Vec<u64> = scene
        .elements_in_viewport(min, max)
        .map(|element| element.id)
        .collect();

    let mut changed = false;
    for id in candidate_ids {
        let Some((path, color, width)) = scene.element(id).map(|element| {
            let width = match element.kind {
                ElementKind::Stroke { width } => width,
                _ => DEFAULT_STROKE_WIDTH,
            };
            (element.path.clone(), element.color, width)
        }) else {
            continue;
        };
        if let Some(remaining) = clip_path_outside_circle(&path, center, radius) {
            changed = true;
            scene.remove_element(id);
            // Surviving pieces of an erased element keep its color and width — erasing
            // part of a red, thick stroke shouldn't turn the rest of it white and thin.
            for piece in remaining {
                scene.add_element_with_color_and_width(piece, color, width);
            }
        }
    }
    changed
}

/// `None` if the circle doesn't actually touch `path` (caller leaves the element alone,
/// unchanged, rather than needlessly removing and re-adding an identical copy under a new
/// id). `Some(pieces)` otherwise — `pieces` is every remaining fragment, possibly empty if
/// the whole element was inside the circle.
fn clip_path_outside_circle(path: &BezPath, center: Point, radius: f64) -> Option<Vec<BezPath>> {
    let (points, closed) = flatten_to_points(path, ERASE_FLATTEN_TOLERANCE);
    if points.len() < 2 {
        return None;
    }
    if !circle_touches_polyline(&points, closed, center, radius) {
        return None;
    }

    let runs = if closed {
        clip_closed_polyline(&points, center, radius)
    } else {
        clip_open_polyline(&points, center, radius)
    };

    Some(
        runs.into_iter()
            .filter(|run| run.len() >= 2)
            .map(|run| path_from_points(&run))
            .collect(),
    )
}

fn is_inside(p: Point, center: Point, radius: f64) -> bool {
    p.distance_squared(center) <= radius * radius
}

/// Whether any part of the polyline (vertex or mid-segment, including the closing edge if
/// `closed`) actually enters the circle — distinct from the R-tree's bounding-box broad
/// phase, which only rules out elements that couldn't possibly intersect, not ones whose
/// path happens to pass near but not through the circle within its own bounding box.
fn circle_touches_polyline(points: &[Point], closed: bool, center: Point, radius: f64) -> bool {
    let edge_count = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    (0..edge_count).any(|i| {
        let p0 = points[i];
        let p1 = points[(i + 1) % points.len()];
        is_inside(p0, center, radius)
            || is_inside(p1, center, radius)
            || !segment_circle_intersections(p0, p1, center, radius).is_empty()
    })
}

/// `t` values in `[0, 1]`, sorted ascending, where the segment `p0`->`p1` crosses the
/// circle's boundary.
fn segment_circle_intersections(p0: Point, p1: Point, center: Point, radius: f64) -> Vec<f64> {
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    let fx = p0.x - center.x;
    let fy = p0.y - center.y;

    let a = dx * dx + dy * dy;
    if a < f64::EPSILON {
        return Vec::new();
    }
    let b = 2.0 * (fx * dx + fy * dy);
    let c = fx * fx + fy * fy - radius * radius;

    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return Vec::new();
    }
    let sqrt_disc = discriminant.sqrt();
    let mut ts = vec![(-b - sqrt_disc) / (2.0 * a), (-b + sqrt_disc) / (2.0 * a)];
    ts.retain(|t| (0.0..=1.0).contains(t));
    ts.sort_by(f64::total_cmp);
    ts
}

/// The core clip: walks an open polyline, emitting every maximal run of it that lies
/// outside the circle (cut exactly at the circle boundary, not at the nearest surviving
/// vertex — see this module's doc comment on why exact segment/circle intersection matters
/// for a straight 2-point line).
fn clip_open_polyline(points: &[Point], center: Point, radius: f64) -> Vec<Vec<Point>> {
    let mut runs = Vec::new();
    let mut current: Vec<Point> = Vec::new();

    if !is_inside(points[0], center, radius) {
        current.push(points[0]);
    }

    for pair in points.windows(2) {
        let (p0, p1) = (pair[0], pair[1]);
        let inside0 = is_inside(p0, center, radius);
        let inside1 = is_inside(p1, center, radius);
        let crossings = segment_circle_intersections(p0, p1, center, radius);

        match (inside0, inside1) {
            (false, false) => {
                if crossings.len() >= 2 {
                    // The segment dips into the circle and back out without either
                    // endpoint being inside it — cut out the dip.
                    let enter = p0.lerp(p1, crossings[0]);
                    let exit = p0.lerp(p1, crossings[crossings.len() - 1]);
                    current.push(enter);
                    runs.push(std::mem::take(&mut current));
                    current.push(exit);
                    current.push(p1);
                } else {
                    current.push(p1);
                }
            }
            (false, true) => {
                // Entering the circle: close out the current run at the boundary.
                let t = crossings.first().copied().unwrap_or(1.0);
                current.push(p0.lerp(p1, t));
                runs.push(std::mem::take(&mut current));
            }
            (true, false) => {
                // Exiting the circle: start a fresh run at the boundary.
                let t = crossings.first().copied().unwrap_or(0.0);
                current.push(p0.lerp(p1, t));
                current.push(p1);
            }
            (true, true) => {
                // Still inside; nothing survives of this segment.
            }
        }
    }

    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// Same idea as [`clip_open_polyline`], but for a closed loop: rotates the point list to
/// start at a vertex known to be outside the circle (so the loop can be cut open there
/// without losing any geometry), clips it as an open polyline including the
/// now-explicit closing edge, then re-joins the first and last resulting runs if they're
/// actually one continuous arc that happened to span the rotation point.
fn clip_closed_polyline(points: &[Point], center: Point, radius: f64) -> Vec<Vec<Point>> {
    let Some(start) = points.iter().position(|&p| !is_inside(p, center, radius)) else {
        // Every vertex is inside the circle — at this flattening resolution, the whole
        // shape is considered erased.
        return Vec::new();
    };

    let mut rotated: Vec<Point> = points[start..].to_vec();
    rotated.extend_from_slice(&points[..start]);
    // Re-close the loop explicitly so the clipper also sees the edge back to the start.
    rotated.push(rotated[0]);

    let mut runs = clip_open_polyline(&rotated, center, radius);

    // If the cut happened to land somewhere other than the rotation point, the arc that
    // survives on one side of it is split into the first and last runs here purely as an
    // artifact of where the rotation opened the loop — rejoin them into one piece.
    if runs.len() >= 2 {
        let first_starts_at_rotation = runs.first().and_then(|r| r.first()) == Some(&rotated[0]);
        let last_ends_at_rotation = runs.last().and_then(|r| r.last()) == Some(&rotated[0]);
        if first_starts_at_rotation && last_ends_at_rotation {
            let mut merged = runs.pop().unwrap();
            let first = runs.remove(0);
            merged.extend(first.into_iter().skip(1));
            runs.insert(0, merged);
        }
    }
    runs
}

fn path_from_points(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    path.move_to(points[0]);
    for &p in &points[1..] {
        path.line_to(p);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape as _;

    fn horizontal_line(x0: f64, x1: f64, y: f64) -> BezPath {
        let mut path = BezPath::new();
        path.move_to((x0, y));
        path.line_to((x1, y));
        path
    }

    fn unit_square() -> BezPath {
        kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01)
    }

    #[test]
    fn erasing_the_middle_of_a_line_splits_it_in_two() {
        let mut scene = Scene::new();
        scene.add_element(horizontal_line(0.0, 10.0, 0.0));

        let changed = erase_circle(&mut scene, DVec2::new(5.0, 0.0), 1.0);

        assert!(changed);
        assert_eq!(scene.len(), 2);
        let mut spans: Vec<(f64, f64)> = scene
            .all_elements()
            .map(|e| {
                let bbox = e.path.bounding_box();
                (bbox.x0, bbox.x1)
            })
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(spans[0].1 < 4.5, "first piece should stop before the gap");
        assert!(spans[1].0 > 5.5, "second piece should start after the gap");
    }

    #[test]
    fn surviving_pieces_keep_the_original_color() {
        let mut scene = Scene::new();
        let red = [0.9, 0.2, 0.2];
        scene.add_element_with_color(horizontal_line(0.0, 10.0, 0.0), red);

        erase_circle(&mut scene, DVec2::new(5.0, 0.0), 1.0);

        assert_eq!(scene.len(), 2);
        assert!(scene.all_elements().all(|e| e.color == red));
    }

    #[test]
    fn fully_covering_an_element_removes_it() {
        let mut scene = Scene::new();
        scene.add_element(horizontal_line(0.0, 1.0, 0.0));

        let changed = erase_circle(&mut scene, DVec2::new(0.5, 0.0), 10.0);

        assert!(changed);
        assert!(scene.is_empty());
    }

    #[test]
    fn a_circle_nowhere_near_the_element_leaves_it_untouched() {
        let mut scene = Scene::new();
        let id = scene.add_element(horizontal_line(0.0, 10.0, 0.0));

        let changed = erase_circle(&mut scene, DVec2::new(100.0, 100.0), 1.0);

        assert!(!changed);
        assert_eq!(scene.len(), 1);
        assert!(scene.element(id).is_some());
    }

    #[test]
    fn erasing_a_corner_of_a_closed_shape_leaves_one_open_arc() {
        let mut scene = Scene::new();
        scene.add_element(unit_square());

        // A circle centered on the (0,0) corner should bite a chunk out of two edges,
        // leaving the rest of the square's outline as one continuous open polyline.
        let changed = erase_circle(&mut scene, DVec2::new(0.0, 0.0), 2.0);

        assert!(changed);
        assert_eq!(scene.len(), 1);
        let remaining = scene.all_elements().next().unwrap();
        // Still one connected piece (not split into two fragments by the rotation cut).
        let bbox = remaining.path.bounding_box();
        assert!(bbox.x1 > 9.0 && bbox.y1 > 9.0);
    }

    #[test]
    fn fully_covering_a_closed_shape_removes_it() {
        let mut scene = Scene::new();
        scene.add_element(unit_square());

        let changed = erase_circle(&mut scene, DVec2::new(5.0, 5.0), 20.0);

        assert!(changed);
        assert!(scene.is_empty());
    }
}
