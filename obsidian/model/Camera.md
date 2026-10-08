# Camera

The camera holds the pan offset and zoom level that define the current view of the
[[Semi_infinite_canvas]].

The camera lives in [[Core_crate]] as pure math (`glam`-based), with no GPU dependency.

The camera clamps zoom to the [[Zoom_range]] (1%-200%).

The camera supports zoom-to-cursor: zooming keeps the board-space point under the cursor
fixed on screen, rather than zooming around the viewport center.

The camera produces the board-space-to-clip-space view matrix that [[Render_crate]]
uploads to the GPU each frame.

The camera is driven by [[App_crate]]'s mouse drag (pan) and scroll (zoom) input handling.
