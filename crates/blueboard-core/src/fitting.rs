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

    fn break_cusp(&self, range: Range<f64>) -> Option<f64> {
        // A fast, sharp reversal *does* happen in real freehand input (doubling back over
        // the same stroke almost instantly) — confirmed the hard way: left unreported, a
        // reversal like this made `fit_to_cubic`'s single-smooth-cubic attempt produce a
        // wildly-overshooting candidate curve whose `CubicBez::arclen` quadrature never
        // converged, hanging the whole app on that one stroke (NaN visible in its
        // accumulator in a debugger backtrace). kurbo's own docs call exactly this out:
        // "might be useful to report near-cusps ... where curvature is large but still
        // mathematically finite" — so detect a strong direction reversal between
        // consecutive segments and report it, rather than relying on kurbo to recover
        // from a cusp it was never told about.
        const REVERSAL_DOT_THRESHOLD: f64 = -0.8;

        let total_length = self.total_length();
        let s_start = range.start * total_length;
        let s_end = range.end * total_length;
        // Strictly inside the range, mirroring the "never at the endpoints" requirement
        // — a tiny margin rather than exact inequality, since `s_start`/`s_end` are
        // themselves earlier-reported (or range-boundary) arc lengths and binary-exact
        // equality isn't guaranteed to survive the multiplication above.
        let margin = total_length * 1e-9;

        for i in 1..self.points.len() - 1 {
            let s = self.cumulative[i];
            if s <= s_start + margin || s >= s_end - margin {
                continue;
            }
            let incoming = self.points[i] - self.points[i - 1];
            let outgoing = self.points[i + 1] - self.points[i];
            if incoming.hypot() <= f64::EPSILON || outgoing.hypot() <= f64::EPSILON {
                continue;
            }
            let (a, b) = (incoming.normalize(), outgoing.normalize());
            let cos_angle = a.x * b.x + a.y * b.y;
            if cos_angle < REVERSAL_DOT_THRESHOLD {
                return Some(s / total_length);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn break_cusp_reports_a_sharp_reversal_strictly_inside_the_range() {
        let points = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(0.0, 0.0),
        ];
        let polyline = Polyline::new(&points).expect("non-degenerate");
        let t = polyline
            .break_cusp(0.0..1.0)
            .expect("a near-180 reversal should be reported");
        assert!(t > 0.0 && t < 1.0);
    }

    #[test]
    fn break_cusp_ignores_a_gentle_turn() {
        let points = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(20.0, 2.0),
        ];
        let polyline = Polyline::new(&points).expect("non-degenerate");
        assert!(polyline.break_cusp(0.0..1.0).is_none());
    }

    #[test]
    fn break_cusp_never_reports_the_ranges_own_endpoints() {
        // A reversal sitting exactly at cumulative length 0 (the range start) must not
        // be reported — kurbo's own doc comment warns this can cause infinite
        // subdivision (report a cusp at a boundary, get called again on a sub-range
        // bounded by that same point, report it again, forever).
        let points = vec![
            DVec2::new(10.0, 0.0),
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
        ];
        let polyline = Polyline::new(&points).expect("non-degenerate");
        // The only reversal vertex is at t=0.5, so narrowing the range to start exactly
        // there must not report it again.
        assert!(polyline.break_cusp(0.5..1.0).is_none());
    }

    /// A stroke that shoots out then immediately doubles back almost exactly over
    /// itself — the shape that, before `break_cusp` detected it, made kurbo's
    /// single-smooth-cubic fit attempt produce a wildly-overshooting candidate curve
    /// whose arc-length computation never converged (confirmed via a debugger backtrace
    /// showing NaN in its accumulator, hanging the whole app on this one stroke).
    #[test]
    fn sharp_reversal_fits_successfully_instead_of_hanging() {
        let points = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(20.0, 0.0),
            DVec2::new(19.9, 0.01),
            DVec2::new(10.0, 0.02),
            DVec2::new(0.0, 0.03),
        ];
        assert!(fit_stroke(&points, 0.5).is_some());
    }
}
