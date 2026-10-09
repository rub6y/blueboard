//! Pan/zoom state and the board-space <-> screen-space math, kept here (not in
//! `blueboard-render`) because it's pure data + logic with no GPU dependency — see
//! PLAN.md's architecture section on why `core` stays headlessly testable.

use glam::{DVec2, Mat4, Vec2, Vec3};

/// 1% zoom, per the brief.
pub const MIN_ZOOM: f64 = 0.01;
/// 400% zoom. The brief originally specified 200%; raised after comparing against Miro
/// directly, which supports noticeably deeper zoom.
pub const MAX_ZOOM: f64 = 4.0;

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// The board-space point currently at the center of the viewport.
    pub pan: DVec2,
    /// Scale factor; 1.0 == 100%.
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: DVec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// Shift the view by a screen-space pixel delta (e.g. a mouse drag), converting it to
    /// board-space by the current zoom so dragging feels 1:1 in screen pixels regardless
    /// of zoom level.
    pub fn pan_by_screen_delta(&mut self, screen_delta: DVec2) {
        self.pan -= screen_delta / self.zoom;
    }

    /// Multiply the zoom by `factor`, clamped to [`MIN_ZOOM`, `MAX_ZOOM`], keeping the
    /// board-space point currently under `anchor_screen` fixed on screen (zoom-to-cursor,
    /// rather than zooming around the viewport center).
    pub fn zoom_by(&mut self, factor: f64, anchor_screen: DVec2, viewport: DVec2) {
        let anchor_board_before = self.screen_to_board(anchor_screen, viewport);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let anchor_board_after = self.screen_to_board(anchor_screen, viewport);
        self.pan += anchor_board_before - anchor_board_after;
    }

    pub fn screen_to_board(&self, screen: DVec2, viewport: DVec2) -> DVec2 {
        (screen - viewport * 0.5) / self.zoom + self.pan
    }

    /// The inverse of [`Camera::screen_to_board`] — board-space to screen-space (physical
    /// pixels, same unit `screen_to_board` takes). Used to place UI drawn outside the
    /// GPU's own camera-transformed pipeline (e.g. `blueboard-render`'s `egui`-painted
    /// scene text) at the right screen position for a given board-space point.
    pub fn board_to_screen(&self, board: DVec2, viewport: DVec2) -> DVec2 {
        (board - self.pan) * self.zoom + viewport * 0.5
    }

    /// Board-space -> clip-space transform for the given viewport size (pixels). Flips Y
    /// (board space is Y-down, like screen space; wgpu clip space is Y-up).
    pub fn view_matrix(&self, viewport: Vec2) -> Mat4 {
        let zoom = self.zoom as f32;
        let pan = self.pan.as_vec2();
        let scale = Mat4::from_scale(Vec3::new(
            2.0 * zoom / viewport.x,
            -2.0 * zoom / viewport.y,
            1.0,
        ));
        let translate = Mat4::from_translation(Vec3::new(-pan.x, -pan.y, 0.0));
        scale * translate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_clamps_to_brief_range() {
        let mut camera = Camera::default();
        camera.zoom_by(1000.0, DVec2::ZERO, DVec2::new(800.0, 600.0));
        assert!((camera.zoom - MAX_ZOOM).abs() < f64::EPSILON);

        camera.zoom_by(0.0001, DVec2::ZERO, DVec2::new(800.0, 600.0));
        assert!((camera.zoom - MIN_ZOOM).abs() < f64::EPSILON);
    }

    #[test]
    fn zoom_keeps_anchor_point_fixed_on_screen() {
        let mut camera = Camera::default();
        let viewport = DVec2::new(800.0, 600.0);
        let anchor_screen = DVec2::new(500.0, 200.0);
        let board_before = camera.screen_to_board(anchor_screen, viewport);

        camera.zoom_by(1.5, anchor_screen, viewport);

        let board_after = camera.screen_to_board(anchor_screen, viewport);
        assert!((board_before - board_after).length() < 1e-9);
    }

    #[test]
    fn board_to_screen_is_the_inverse_of_screen_to_board() {
        let camera = Camera {
            pan: DVec2::new(37.0, -12.0),
            zoom: 2.5,
        };
        let viewport = DVec2::new(800.0, 600.0);
        let screen = DVec2::new(321.0, 456.0);

        let board = camera.screen_to_board(screen, viewport);
        let round_tripped = camera.board_to_screen(board, viewport);

        assert!((screen - round_tripped).length() < 1e-9);
    }
}
