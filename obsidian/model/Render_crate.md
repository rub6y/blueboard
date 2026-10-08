# Render crate

The render crate (`blueboard-render`) holds the wgpu rendering pipeline.

The render crate performs [[Tessellation]] of [[Bezier_curve]] strokes into GPU meshes.

The render crate performs [[Viewport_culling]] using the [[RTree_index]].

The render crate applies [[LOD_simplification]] at low zoom levels.

The render crate depends on [[Core_crate]] and knows nothing about UI widgets.
