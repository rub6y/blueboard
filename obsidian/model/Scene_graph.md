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
