# R-tree index

The R-tree index is the spatial index over the [[Scene_graph]]'s element bounding boxes.

The R-tree index is implemented with the `rstar` crate rather than hand-rolled.

The R-tree index answers viewport queries in expected O(log n + k) time instead of the
O(n) a linear scan would cost.

The R-tree index is used by [[Viewport_culling]].

The math behind the R-tree query is written up in `planning/math.tex` (see
[[planning_RESEARCH]]).
