# Tessellation

Tessellation converts [[Bezier_curve]] paths into triangles the GPU can render.

Tessellation is performed by the `lyon` crate in [[Render_crate]].

Tessellation flattens curves adaptively, subdividing until a screen-space flatness
tolerance is met, so detail scales with zoom level automatically.

Tessellated meshes are cached per element and re-tessellated only on edit or LOD-bucket
change, not every frame.

Tessellation work is offloaded to worker threads via `rayon` to keep it off the render
thread.
