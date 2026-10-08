# Tessellation

Tessellation converts [[Bezier_curve]] paths into triangles the GPU can render.

Tessellation is performed by the `lyon` crate in [[Render_crate]].

Tessellation flattens curves adaptively, subdividing until a screen-space flatness
tolerance is met (the tolerance is divided by the [[Camera]]'s zoom before being passed to
`lyon`, so detail scales with zoom level automatically).

Tessellation currently re-tessellates every visible stroke every frame, on the main
thread — caching tessellated meshes per element and offloading the work to `rayon` are
both still open (PLAN.md Phase 6 performance pass), not yet needed at Phase 2's scale.

Tessellation output is rendered through a 4x MSAA target resolved into the swapchain, to
anti-alias triangle edges — added in Phase 2 after a side-by-side comparison against
Miro showed visibly jagged stroke edges without it.
