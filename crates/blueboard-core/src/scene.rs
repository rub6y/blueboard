//! The scene graph: the flat list of elements making up a board. Phase 2 only has
//! strokes; shapes/text/groups arrive in later phases per PLAN.md.

use std::collections::HashMap;

use glam::DVec2;
use kurbo::{BezPath, Shape};
use rstar::{RTree, RTreeObject, AABB};

pub struct Stroke {
    pub id: u64,
    pub path: BezPath,
}

/// What's actually indexed by the R-tree: just an id and a bounding box, not the stroke
/// itself — the strokes live in `Scene::strokes`, looked up by id after a spatial query.
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
    strokes: HashMap<u64, Stroke>,
    index: RTree<IndexedBounds>,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_stroke(&mut self, path: BezPath) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.insert_stroke(id, path);
        id
    }

    /// Inserts a stroke under a specific id rather than generating one — used when
    /// loading a saved board ([`crate::file_format`]), so loaded strokes keep the ids
    /// they were saved with instead of being renumbered.
    pub(crate) fn insert_loaded_stroke(&mut self, id: u64, path: BezPath) {
        self.insert_stroke(id, path);
        // Any strokes added after loading must not collide with a loaded id.
        self.next_id = self.next_id.max(id + 1);
    }

    fn insert_stroke(&mut self, id: u64, path: BezPath) {
        let bbox = path.bounding_box();
        let envelope = AABB::from_corners([bbox.x0, bbox.y0], [bbox.x1, bbox.y1]);
        self.index.insert(IndexedBounds { id, envelope });
        self.strokes.insert(id, Stroke { id, path });
    }

    /// Strokes whose bounding box intersects the given board-space rectangle — the
    /// viewport-culling query `blueboard-render` runs every frame (PLAN.md Phase 2: wired
    /// in now even though boards are still small).
    pub fn strokes_in_viewport(&self, min: DVec2, max: DVec2) -> impl Iterator<Item = &Stroke> {
        let envelope = AABB::from_corners([min.x, min.y], [max.x, max.y]);
        self.index
            .locate_in_envelope_intersecting(envelope)
            .filter_map(move |entry| self.strokes.get(&entry.id))
    }

    /// Every stroke in the scene, regardless of position — used by
    /// [`crate::file_format`] to save the whole board, not just what's on screen.
    pub fn all_strokes(&self) -> impl Iterator<Item = &Stroke> {
        self.strokes.values()
    }

    pub fn len(&self) -> usize {
        self.strokes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }
}
