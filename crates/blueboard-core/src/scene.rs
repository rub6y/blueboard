//! The scene graph: the flat list of elements making up a board. Through Phase 4 the only
//! element kind was a pen stroke; Phase 5 adds shapes (rect/ellipse/line), all represented
//! the same way — a [`kurbo::BezPath`] outline — so they share storage, indexing, and
//! rendering with no extra cases.

use std::collections::HashMap;

use glam::DVec2;
use kurbo::{BezPath, ParamCurveNearest, Point, Shape as _};
use rstar::{RTree, RTreeObject, AABB};

use crate::history::Snapshot;

pub struct Element {
    pub id: u64,
    pub path: BezPath,
    pub color: [f32; 3],
}

/// What a new element gets if nothing more specific is given (`add_element`, and any
/// loaded element from a pre-color file format version). White, matching the fixed
/// stroke color every element used before per-element color existed.
pub const DEFAULT_ELEMENT_COLOR: [f32; 3] = [1.0, 1.0, 1.0];

/// What's actually indexed by the R-tree: just an id and a bounding box, not the element
/// itself — elements live in `Scene::elements`, looked up by id after a spatial query.
#[derive(PartialEq)]
struct IndexedBounds {
    id: u64,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for IndexedBounds {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

#[derive(Default)]
pub struct Scene {
    next_id: u64,
    elements: HashMap<u64, Element>,
    index: RTree<IndexedBounds>,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an element in [`DEFAULT_ELEMENT_COLOR`] — most of this crate's own tests use
    /// this, since they don't care about color. Drawing tools that know what color the
    /// user actually picked use [`Scene::add_element_with_color`] instead.
    pub fn add_element(&mut self, path: BezPath) -> u64 {
        self.add_element_with_color(path, DEFAULT_ELEMENT_COLOR)
    }

    pub fn add_element_with_color(&mut self, path: BezPath, color: [f32; 3]) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.insert_element(id, path, color);
        id
    }

    /// Inserts an element under a specific id rather than generating one — used when
    /// loading a saved board ([`crate::file_format`]), so loaded elements keep the ids
    /// they were saved with instead of being renumbered.
    pub(crate) fn insert_loaded_element(&mut self, id: u64, path: BezPath, color: [f32; 3]) {
        self.insert_element(id, path, color);
        // Any elements added after loading must not collide with a loaded id.
        self.next_id = self.next_id.max(id + 1);
    }

    fn insert_element(&mut self, id: u64, path: BezPath, color: [f32; 3]) {
        let bbox = path.bounding_box();
        let envelope = AABB::from_corners([bbox.x0, bbox.y0], [bbox.x1, bbox.y1]);
        self.index.insert(IndexedBounds { id, envelope });
        self.elements.insert(id, Element { id, path, color });
    }

    pub fn element(&self, id: u64) -> Option<&Element> {
        self.elements.get(&id)
    }

    /// Removes an element entirely, from both the map and the R-tree. Used by the
    /// whole-object eraser tool directly, and by [`crate::erase`] to drop an element
    /// before re-adding whatever survives a partial erase as new elements.
    pub fn remove_element(&mut self, id: u64) -> bool {
        let Some(element) = self.elements.remove(&id) else {
            return false;
        };
        let bbox = element.path.bounding_box();
        let envelope = AABB::from_corners([bbox.x0, bbox.y0], [bbox.x1, bbox.y1]);
        self.index.remove(&IndexedBounds { id, envelope });
        true
    }

    /// Replaces an existing element's path in place (its id and position in `elements`
    /// are unchanged) — used by the selection tool to commit a drag. The R-tree entry is
    /// removed and reinserted rather than mutated, since a move changes the bounding box
    /// the old entry was indexed under.
    pub fn set_element_path(&mut self, id: u64, path: BezPath) {
        // A move doesn't change what color the element was drawn in — read it back from
        // the existing entry (falling back to the default for an id that, degenerately,
        // doesn't exist yet) rather than letting `insert_element` reset it.
        let color = self
            .elements
            .get(&id)
            .map(|element| element.color)
            .unwrap_or(DEFAULT_ELEMENT_COLOR);
        if let Some(old) = self.elements.get(&id) {
            let old_bbox = old.path.bounding_box();
            let old_envelope =
                AABB::from_corners([old_bbox.x0, old_bbox.y0], [old_bbox.x1, old_bbox.y1]);
            self.index.remove(&IndexedBounds {
                id,
                envelope: old_envelope,
            });
        }
        self.insert_element(id, path, color);
    }

    /// The id of the element nearest `point` (board-space), if any lie within `tolerance`
    /// of it — used by the selection tool to turn a click into "what did the user click
    /// on." Distance is to the path's *outline*, not its filled interior: every element
    /// renders as a stroke (see `blueboard-render::tessellate_stroke`), even shapes that
    /// look "filled", so this matches what's actually visible rather than treating
    /// rect/ellipse as hit-testable by interior containment.
    ///
    /// Ties (two elements equally close, e.g. overlapping) resolve to the nearer one;
    /// exact ties fall back to whichever the R-tree query happens to visit last, which in
    /// practice is fine for a feature this is — a perfect "topmost wins" ordering would
    /// need a z-order the scene doesn't track yet.
    pub fn hit_test(&self, point: DVec2, tolerance: f64) -> Option<u64> {
        let query = AABB::from_corners(
            [point.x - tolerance, point.y - tolerance],
            [point.x + tolerance, point.y + tolerance],
        );
        let candidate_point = Point::new(point.x, point.y);
        self.index
            .locate_in_envelope_intersecting(query)
            .filter_map(|entry| {
                let element = self.elements.get(&entry.id)?;
                let distance = element
                    .path
                    .segments()
                    .map(|segment| segment.nearest(candidate_point, 0.1).distance_sq)
                    .fold(f64::INFINITY, f64::min)
                    .sqrt();
                (distance <= tolerance).then_some((entry.id, distance))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(id, _)| id)
    }

    /// A point-in-time copy of this scene's contents for [`crate::history`]. Derived data
    /// (the R-tree) is not copied — [`Scene::restore`] rebuilds it from the elements.
    pub fn snapshot(&self) -> Snapshot {
        let mut elements: Vec<(u64, BezPath, [f32; 3])> = self
            .elements
            .values()
            .map(|element| (element.id, element.path.clone(), element.color))
            .collect();
        // Sorted by id so the same elements always compare equal regardless of `HashMap`
        // iteration order — same reasoning as `file_format::save`.
        elements.sort_by_key(|(id, _, _)| *id);
        Snapshot { elements }
    }

    /// Replaces this scene's contents with `snapshot`'s — how [`crate::history`] applies
    /// an undo/redo.
    pub(crate) fn restore(&mut self, snapshot: Snapshot) {
        // Both the element map *and* the index are rebuilt: a stale index entry left
        // behind by a removed element would make `elements_in_viewport` report an id
        // whose bounding box no longer matches the element it finds.
        self.elements.clear();
        self.index = RTree::new();
        // `next_id` is never lowered, only raised past the highest restored id: ids are
        // never reused (the same stability rule `insert_loaded_element` follows), and
        // lowering it after e.g. undoing a `clear` would collide with the restored ids.
        if let Some(highest) = snapshot.elements.iter().map(|(id, _, _)| *id).max() {
            self.next_id = self.next_id.max(highest + 1);
        }
        for (id, path, color) in snapshot.elements {
            self.insert_element(id, path, color);
        }
    }

    /// Elements whose bounding box intersects the given board-space rectangle — the
    /// viewport-culling query `blueboard-render` runs every frame (PLAN.md Phase 2: wired
    /// in now even though boards are still small).
    pub fn elements_in_viewport(&self, min: DVec2, max: DVec2) -> impl Iterator<Item = &Element> {
        let envelope = AABB::from_corners([min.x, min.y], [max.x, max.y]);
        self.index
            .locate_in_envelope_intersecting(envelope)
            .filter_map(move |entry| self.elements.get(&entry.id))
    }

    /// Every element in the scene, regardless of position — used by
    /// [`crate::file_format`] to save the whole board, not just what's on screen.
    pub fn all_elements(&self) -> impl Iterator<Item = &Element> {
        self.elements.values()
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A horizontal line from `(x0, y)` to `(x1, y)` — easy to reason about distances to.
    fn horizontal_line(x0: f64, x1: f64, y: f64) -> BezPath {
        let mut path = BezPath::new();
        path.move_to((x0, y));
        path.line_to((x1, y));
        path
    }

    #[test]
    fn hit_test_finds_an_element_within_tolerance() {
        let mut scene = Scene::new();
        let id = scene.add_element(horizontal_line(0.0, 10.0, 0.0));

        assert_eq!(scene.hit_test(DVec2::new(5.0, 1.0), 2.0), Some(id));
    }

    #[test]
    fn hit_test_misses_past_tolerance() {
        let mut scene = Scene::new();
        scene.add_element(horizontal_line(0.0, 10.0, 0.0));

        assert_eq!(scene.hit_test(DVec2::new(5.0, 10.0), 2.0), None);
    }

    #[test]
    fn hit_test_picks_the_nearer_of_two_overlapping_elements() {
        let mut scene = Scene::new();
        let far = scene.add_element(horizontal_line(0.0, 10.0, 0.0));
        let near = scene.add_element(horizontal_line(0.0, 10.0, 1.0));

        let hit = scene.hit_test(DVec2::new(5.0, 1.4), 5.0);
        assert_eq!(hit, Some(near));
        assert_ne!(hit, Some(far));
    }

    #[test]
    fn set_element_path_moves_the_indexed_bounding_box() {
        let mut scene = Scene::new();
        let id = scene.add_element(horizontal_line(0.0, 10.0, 0.0));

        scene.set_element_path(id, horizontal_line(100.0, 110.0, 100.0));

        // The element no longer intersects its old location...
        let old_location: Vec<_> = scene
            .elements_in_viewport(DVec2::new(-5.0, -5.0), DVec2::new(15.0, 5.0))
            .map(|element| element.id)
            .collect();
        assert!(old_location.is_empty());
        // ...and does intersect its new one, via the same index query — proving the old
        // R-tree entry was actually removed, not just shadowed by the new one.
        let new_location: Vec<_> = scene
            .elements_in_viewport(DVec2::new(95.0, 95.0), DVec2::new(115.0, 105.0))
            .map(|element| element.id)
            .collect();
        assert_eq!(new_location, vec![id]);
    }
}
