//! Resize-handle geometry shared between `blueboard-app` (hit-testing a click against a
//! handle, and the resize math itself) and `blueboard-render` (drawing the handle
//! affordances) — one source of truth for where the eight handles sit around a
//! selection's bounding box, so the two can't drift out of sync with each other. Pure
//! geometry, no scene/tool/GPU concerns, per this crate's own "stays independent" style
//! (see `camera.rs`'s doc comment).

use kurbo::{Affine, Point, Rect};

/// Minimum width/height a resize can shrink a selection's bounding box to — prevents a
/// drag past the opposite handle from collapsing it to zero or negative size.
const MIN_SIZE: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl ResizeHandle {
    /// The point that must stay fixed as a drag on this handle proceeds — the opposite
    /// corner/edge. E.g. dragging `BottomRight` anchors `rect`'s top-left corner.
    pub fn anchor(self, rect: Rect) -> Point {
        let mx = (rect.x0 + rect.x1) / 2.0;
        let my = (rect.y0 + rect.y1) / 2.0;
        match self {
            ResizeHandle::TopLeft => Point::new(rect.x1, rect.y1),
            ResizeHandle::Top => Point::new(mx, rect.y1),
            ResizeHandle::TopRight => Point::new(rect.x0, rect.y1),
            ResizeHandle::Right => Point::new(rect.x0, my),
            ResizeHandle::BottomRight => Point::new(rect.x0, rect.y0),
            ResizeHandle::Bottom => Point::new(mx, rect.y0),
            ResizeHandle::BottomLeft => Point::new(rect.x1, rect.y0),
            ResizeHandle::Left => Point::new(rect.x1, my),
        }
    }

    /// Whether dragging this handle changes the rect's width — an edge handle (`Top`/
    /// `Bottom`) only changes height, a corner or `Left`/`Right` handle changes width.
    fn affects_x(self) -> bool {
        !matches!(self, ResizeHandle::Top | ResizeHandle::Bottom)
    }

    /// Whether dragging this handle changes the rect's height — the `affects_x` mirror.
    fn affects_y(self) -> bool {
        !matches!(self, ResizeHandle::Left | ResizeHandle::Right)
    }
}

/// The eight resize-handle positions (board-space) around `rect`'s corners and edge
/// midpoints.
pub fn handle_points(rect: Rect) -> [(ResizeHandle, Point); 8] {
    let mx = (rect.x0 + rect.x1) / 2.0;
    let my = (rect.y0 + rect.y1) / 2.0;
    [
        (ResizeHandle::TopLeft, Point::new(rect.x0, rect.y0)),
        (ResizeHandle::Top, Point::new(mx, rect.y0)),
        (ResizeHandle::TopRight, Point::new(rect.x1, rect.y0)),
        (ResizeHandle::Right, Point::new(rect.x1, my)),
        (ResizeHandle::BottomRight, Point::new(rect.x1, rect.y1)),
        (ResizeHandle::Bottom, Point::new(mx, rect.y1)),
        (ResizeHandle::BottomLeft, Point::new(rect.x0, rect.y1)),
        (ResizeHandle::Left, Point::new(rect.x0, my)),
    ]
}

/// The new bounding box while dragging `handle` (originally at `original`'s corresponding
/// corner/edge) to `current_point` — the handle's anchor stays fixed, the dragged side
/// follows `current_point` (clamped to [`MIN_SIZE`], and correctly flips if dragged past
/// the anchor rather than producing an inverted rect).
pub fn resized_rect(handle: ResizeHandle, original: Rect, current_point: Point) -> Rect {
    let anchor = handle.anchor(original);
    let (x0, x1) = if handle.affects_x() {
        let width = (current_point.x - anchor.x).abs().max(MIN_SIZE);
        if current_point.x >= anchor.x {
            (anchor.x, anchor.x + width)
        } else {
            (anchor.x - width, anchor.x)
        }
    } else {
        (original.x0, original.x1)
    };
    let (y0, y1) = if handle.affects_y() {
        let height = (current_point.y - anchor.y).abs().max(MIN_SIZE);
        if current_point.y >= anchor.y {
            (anchor.y, anchor.y + height)
        } else {
            (anchor.y - height, anchor.y)
        }
    } else {
        (original.y0, original.y1)
    };
    Rect::new(x0, y0, x1, y1)
}

/// The affine transform mapping `original`'s content to fill `resized` — used to scale an
/// element's path (any kind) from its pre-drag bounding box to the dragged one.
pub fn scale_affine(original: Rect, resized: Rect) -> Affine {
    let sx = if original.width() > f64::EPSILON {
        resized.width() / original.width()
    } else {
        1.0
    };
    let sy = if original.height() > f64::EPSILON {
        resized.height() / original.height()
    } else {
        1.0
    };
    Affine::translate((resized.x0, resized.y0))
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate((-original.x0, -original.y0))
}

/// The geometric-mean scale factor between `original` and `resized` — used to scale a
/// text element's font size by "how much bigger/smaller overall," a single number, even
/// though the box itself may have resized non-uniformly (e.g. only an edge handle
/// dragged, changing just one axis).
pub fn uniform_scale_factor(original: Rect, resized: Rect) -> f64 {
    let sx = if original.width() > f64::EPSILON {
        resized.width() / original.width()
    } else {
        1.0
    };
    let sy = if original.height() > f64::EPSILON {
        resized.height() / original.height()
    } else {
        1.0
    };
    (sx * sy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_bottom_right_anchors_top_left() {
        let original = Rect::new(0.0, 0.0, 10.0, 10.0);
        let resized = resized_rect(ResizeHandle::BottomRight, original, Point::new(20.0, 15.0));
        assert_eq!(resized, Rect::new(0.0, 0.0, 20.0, 15.0));
    }

    #[test]
    fn dragging_top_left_anchors_bottom_right() {
        let original = Rect::new(0.0, 0.0, 10.0, 10.0);
        let resized = resized_rect(ResizeHandle::TopLeft, original, Point::new(2.0, 3.0));
        assert_eq!(resized, Rect::new(2.0, 3.0, 10.0, 10.0));
    }

    #[test]
    fn dragging_past_the_anchor_flips_instead_of_inverting() {
        let original = Rect::new(0.0, 0.0, 10.0, 10.0);
        // Dragging the right edge to the left of the (left-anchored) x0.
        let resized = resized_rect(ResizeHandle::Right, original, Point::new(-5.0, 0.0));
        assert_eq!(resized.x0, -5.0);
        assert_eq!(resized.x1, 0.0);
        assert!(resized.x0 < resized.x1);
    }

    #[test]
    fn edge_handle_only_changes_its_own_axis() {
        let original = Rect::new(0.0, 0.0, 10.0, 10.0);
        let resized = resized_rect(ResizeHandle::Right, original, Point::new(30.0, 999.0));
        assert_eq!(resized.y0, original.y0);
        assert_eq!(resized.y1, original.y1);
    }

    #[test]
    fn resize_never_collapses_below_min_size() {
        let original = Rect::new(0.0, 0.0, 10.0, 10.0);
        let resized = resized_rect(ResizeHandle::BottomRight, original, Point::new(0.1, 0.1));
        assert!(resized.width() >= 2.0);
        assert!(resized.height() >= 2.0);
    }

    #[test]
    fn uniform_scale_factor_is_one_for_an_unchanged_rect() {
        let rect = Rect::new(0.0, 0.0, 10.0, 20.0);
        assert!((uniform_scale_factor(rect, rect) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn uniform_scale_factor_is_geometric_mean_of_both_axes() {
        let original = Rect::new(0.0, 0.0, 10.0, 10.0);
        let resized = Rect::new(0.0, 0.0, 20.0, 40.0); // 2x width, 4x height
        let factor = uniform_scale_factor(original, resized);
        assert!((factor - 8f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn scale_affine_maps_original_corners_onto_resized_corners() {
        let original = Rect::new(0.0, 0.0, 10.0, 10.0);
        let resized = Rect::new(5.0, 5.0, 25.0, 15.0);
        let affine = scale_affine(original, resized);
        let top_left = affine * Point::new(original.x0, original.y0);
        let bottom_right = affine * Point::new(original.x1, original.y1);
        assert!((top_left.x - resized.x0).abs() < 1e-9 && (top_left.y - resized.y0).abs() < 1e-9);
        assert!(
            (bottom_right.x - resized.x1).abs() < 1e-9
                && (bottom_right.y - resized.y1).abs() < 1e-9
        );
    }
}
