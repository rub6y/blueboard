# Bezier curve

A Bezier curve is the geometric primitive used to represent [[Vector_pen]] strokes.

Blueboard stores strokes as cubic Bezier paths using the `kurbo` crate.

Bezier curves are fit from raw pointer input using Schneider's curve-fitting method
(parameterize by arc length, least-squares fit, subdivide where error exceeds tolerance).

Bezier curves are flattened into polylines for GPU rendering by [[Tessellation]].

Bezier curves are evaluated and split using De Casteljau's algorithm.

The math behind Bezier curves is written up in `planning/math.tex` (see
[[planning_RESEARCH]]).
