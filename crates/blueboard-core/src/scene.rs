//! The scene graph: the flat list of elements making up a board. Through Phase 4 the only
//! element kind was a pen stroke; Phase 5 adds shapes (rect/ellipse/line), all represented
//! the same way — a [`kurbo::BezPath`] outline — so they share storage, indexing, and
//! rendering with no extra cases.

use std::collections::HashMap;

use glam::DVec2;
use kurbo::{BezPath, Shape as _};
use rstar::{RTree, RTreeObject, AABB};

pub struct Element {
    pub id: u64,
    pub path: BezPath,
}

/// What's actually indexed by the R-tree: just an id and a bounding box, not the element
/// itself — elements live in `Scene::elements`, looked up by id after a spatial query.
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

    pub fn add_element(&mut self, path: BezPath) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.insert_element(id, path);
        id
    }

    /// Inserts an element under a specific id rather than generating one — used when
    /// loading a saved board ([`crate::file_format`]), so loaded elements keep the ids
    /// they were saved with instead of being renumbered.
    pub(crate) fn insert_loaded_element(&mut self, id: u64, path: BezPath) {
        self.insert_element(id, path);
        // Any elements added after loading must not collide with a loaded id.
        self.next_id = self.next_id.max(id + 1);
    }

    fn insert_element(&mut self, id: u64, path: BezPath) {
        let bbox = path.bounding_box();
        let envelope = AABB::from_corners([bbox.x0, bbox.y0], [bbox.x1, bbox.y1]);
        self.index.insert(IndexedBounds { id, envelope });
        self.elements.insert(id, Element { id, path });
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
