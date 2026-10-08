# LOD simplification

LOD simplification reduces stroke detail when an element's on-screen size drops below a
pixel threshold at low [[Zoom_range]] levels.

LOD simplification uses the Ramer-Douglas-Peucker algorithm on the flattened polyline
produced by [[Tessellation]].

LOD simplification results are cached per LOD bucket alongside the full-detail
tessellation.

LOD simplification is planned for Phase 6 of [[planning_PLAN]] ("performance pass"), added
only once a real board exists to profile against.
