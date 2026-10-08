# Viewport culling

Viewport culling restricts rendering to only the [[Scene_graph]] elements intersecting
the current visible viewport.

Viewport culling is what keeps the [[Semi_infinite_canvas]] cheap to render regardless of
total board size.

Viewport culling is implemented by querying the [[RTree_index]].
