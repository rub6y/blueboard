# Fitting

Fitting converts raw [[Vector_pen]] pointer samples into a smooth [[Bezier_curve]] path.

Fitting wraps the raw samples as a `kurbo::ParamCurveFit` source — a piecewise-linear
polyline parameterized by normalized cumulative arc length — and reuses kurbo's own
`fit_to_bezpath`, rather than hand-implementing Schneider's algorithm.

Fitting's accuracy tolerance is converted from screen-space pixels to board-space units by
dividing by the [[Camera]]'s zoom, so a stroke looks equally precise regardless of the zoom
level it was drawn at.

Fitting lives in [[Core_crate]].
