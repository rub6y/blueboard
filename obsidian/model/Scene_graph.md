# Scene graph

The scene graph is the flat list of elements (strokes, shapes, text, groups) that make up
a board.

Each element in the scene graph has a stable ID, a transform, a style, and a geometry.

Strokes in the scene graph are stored as [[Bezier_curve]] paths, not rasterized, so zoom
stays lossless across the [[Zoom_range]].

The scene graph is indexed by the [[RTree_index]] for [[Viewport_culling]].

The scene graph lives in [[Core_crate]].

The scene graph is serialized to disk by the [[File_format]].

As of Phase 5's selection tool, the scene graph also answers `hit_test` (nearest element
to a point, within a tolerance, by distance to the path's rendered outline — not interior
containment, since every element renders as a stroke) and `set_element_path` (replaces an
element's path in place, reindexing its [[RTree_index]] entry) — see [[Tool_registry]].

As of Phase 5's eraser work, the scene graph also answers `remove_element` (deletes an
element from both the map and the [[RTree_index]]) and, via the `erase` module built on
top of it, `erase_circle` (subtracts a circular region from every element it touches —
real segment/circle clipping on a flattened polyline, not just nearest-vertex removal —
splitting strokes and shapes into whatever survives outside the circle).

Each element now also carries a `color: [f32; 3]`, set by [[Command_registry]]'s
`:color` command via [[Tool_registry]]'s `ToolContext`. `set_element_path` (the move
commit) preserves an element's existing color rather than resetting it; `erase_circle`
carries an erased element's color through to every surviving split piece. Undo/redo's
`Snapshot` had to grow a color field too, for the same reason — without it, every undo
would silently strip color back to default.
