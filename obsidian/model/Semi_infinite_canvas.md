# Semi-infinite canvas

The semi-infinite canvas is Blueboard's board space: large but not unbounded, stored as
`f64` board-space coordinates.

The semi-infinite canvas is panned and zoomed via a single view transform applied at
render time, with geometry never rebaked into screen-space.

The semi-infinite canvas stays cheap to render via [[Viewport_culling]].

The semi-infinite canvas supports the [[Zoom_range]].
