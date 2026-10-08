//! Freehand pointer samples -> a smooth vector [`BezPath`], per `planning/math.tex`'s
//! curve-fitting section (arc-length parameterization, error-bounded subdivision).
//!
//! Rather than reimplement that algorithm (classically, Schneider's method) by hand, this
//! wraps the raw samples as a [`kurbo::ParamCurveFit`] source — a piecewise-linear "curve"
//! parameterized by normalized cumulative arc length — and hands it to kurbo's own
//! `fit_to_bezpath`. Same idea as the math note, reusing a maintained implementation
//! instead of a hand-rolled one.

use std::ops::Range;

use glam::DVec2;
use kurbo::{fit_to_bezpath, BezPath, CurveFitSample, ParamCurveFit, Point, Vec2};

struct Polyline {
    points: Vec<Point>,
    /// Cumulative arc length up to each point; `cumulative[0] == 0.0`,
    /// `cumulative[last] == total length`. Same length as `points`.
    cumulative: Vec<f64>,
}

impl Polyline {
    fn new(points: &[DVec2]) -> Option<Self> {
        let points: Vec<Point> = points.iter().map(|p| Point::new(p.x, p.y)).collect();

        let mut cumulative = Vec::with_capacity(points.len());
        cumulative.push(0.0);
        for pair in points.windows(2) {
            let length = pair[0].distance(pair[1]);
            cumulative.push(cumulative.last().unwrap() + length);
        }

        if *cumulative.last()? <= f64::EPSILON {
            // All samples coincide (e.g. a click with no drag) — nothing to fit.
            return None;
        }

        Some(Self { points, cumulative })
    }

    fn total_length(&self) -> f64 {
        *self.cumulative.last().expect("non-empty by construction")
    }

    /// The point and the (non-unit) direction of the segment containing arc length `s`,
    /// biased towards the segment before a breakpoint when `sign` is negative — so a
    /// sample taken exactly at a vertex still gets a well-defined tangent on each side.
    fn sample(&self, s: f64, sign: f64) -> (Point, Vec2) {
        let mut i = match self
            .cumulative
            .binary_search_by(|len| len.partial_cmp(&s).unwrap())
        {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        i = i.min(self.points.len() - 2);
        if sign < 0.0 && i > 0 && (self.cumulative[i] - s).abs() < f64::EPSILON {
            i -= 1;
        }

        let (p0, p1) = (self.points[i], self.points[i + 1]);
        let segment_length = self.cumulative[i + 1] - self.cumulative[i];
        let local_t = if segment_length > f64::EPSILON {
            (s - self.cumulative[i]) / segment_length
        } else {
            0.0
        };

        (p0.lerp(p1, local_t), p1 - p0)
    }
}

impl ParamCurveFit for Polyline {
    fn sample_pt_tangent(&self, t: f64, sign: f64) -> CurveFitSample {
        let s = t.clamp(0.0, 1.0) * self.total_length();
        let (p, direction) = self.sample(s, sign);
        let tangent = if direction.hypot() > f64::EPSILON {
            direction.normalize()
        } else {
            Vec2::new(1.0, 0.0)
        };
        CurveFitSample { p, tangent }
    }

    fn sample_pt_deriv(&self, t: f64) -> (Point, Vec2) {
        let s = t.clamp(0.0, 1.0) * self.total_length();
        let (p, direction) = self.sample(s, 1.0);
        // t is s/total_length, so dPoint/dt = dPoint/ds * total_length (chain rule).
        let segment_length = direction.hypot();
        let deriv = if segment_length > f64::EPSILON {
            direction / segment_length * self.total_length()
        } else {
            Vec2::ZERO
        };
        (p, deriv)
    }

    fn break_cusp(&self, _range: Range<f64>) -> Option<f64> {
        // Freehand pen strokes essentially never have a true cusp (zero-derivative point);
        // omitting detection just costs a couple of extra segments at a sharp corner
        // instead of splitting exactly there, which kurbo's own docs call an accepted
        // fallback rather than a correctness problem.
        None
    }
}

/// Fit `points` (board-space pointer samples, in drawing order) to a smooth [`BezPath`],
/// accurate to within `accuracy` board-space units. Returns `None` for input too short or
/// too degenerate to fit (a click with no drag, or every sample landing on one point).
pub fn fit_stroke(points: &[DVec2], accuracy: f64) -> Option<BezPath> {
    if points.len() < 2 {
        return None;
    }
    let polyline = Polyline::new(points)?;
    Some(fit_to_bezpath(&polyline, accuracy.max(1e-6)))
}
