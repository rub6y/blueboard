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

        let bbox = path.bounding_box();
        let envelope = AABB::from_corners([bbox.x0, bbox.y0], [bbox.x1, bbox.y1]);
        self.index.insert(IndexedBounds { id, envelope });
        self.strokes.insert(id, Stroke { id, path });

        id
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
}
