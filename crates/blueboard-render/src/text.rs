//! Paints a [`TextRun`] sequence with `egui`'s painter: bold via a faux-bold double-paint
//! (this app's fonts have no separate bold weight), and a compiled LaTeX math span
//! (`TextRun::Image`) as a scaled, aspect-preserved image — `lib.rs`'s `render` resolves
//! `text_markup::MathToken::Math` into one of these from `Renderer::math_textures` before
//! calling here, so this module never deals with LaTeX/compilation itself, only painting.
//! Deliberately single-line: no wrapping, matching `text_markup::parse`'s own scope.

use egui::{Color32, FontId, Painter, Pos2, Rect, Vec2};

/// How many extra times a faux-bold run is painted, each offset by one more pixel — see
/// this module's doc comment.
const FAUX_BOLD_PASSES: i32 = 2;
/// A math image is scaled so its height matches `font_size * IMAGE_HEIGHT_RATIO` (aspect
/// ratio preserved) — matches plain text's own line-height ratio elsewhere in this crate,
/// so a math span reads as roughly the same visual size as the surrounding text.
const IMAGE_HEIGHT_RATIO: f32 = 1.3;

/// One resolved piece of a text element, ready to paint — see this module's doc comment
/// on where `Image` comes from.
pub enum TextRun {
    Text {
        text: String,
        bold: bool,
    },
    Image {
        texture_id: egui::TextureId,
        /// Native pixel size, used to preserve aspect ratio when scaled to `font_size`.
        width: f32,
        height: f32,
    },
}

/// Paints `runs` left-to-right starting at `top_left` (the text element's board-space
/// top-left corner, already converted to screen/logical points by the caller).
pub fn paint_runs(
    painter: &Painter,
    top_left: Pos2,
    runs: &[TextRun],
    font_size: f32,
    color: Color32,
) {
    let baseline_y = top_left.y + font_size;
    let mut cursor_x = top_left.x;

    for run in runs {
        match run {
            TextRun::Text { text, bold } => {
                let font = FontId::proportional(font_size);
                let size = paint_run(
                    painter,
                    Pos2::new(cursor_x, baseline_y - font_size),
                    text,
                    font,
                    color,
                    *bold,
                );
                cursor_x += size.x;
            }
            TextRun::Image {
                texture_id,
                width,
                height,
            } => {
                let draw_height = font_size * IMAGE_HEIGHT_RATIO;
                let draw_width = width * (draw_height / height.max(1.0));
                // Centered around the text's vertical midline rather than baseline-aligned
                // — a math image (a fraction, say) typically extends both above and below
                // where plain text's baseline would sit.
                let y = baseline_y - font_size * 0.5 - draw_height * 0.5;
                let rect =
                    Rect::from_min_size(Pos2::new(cursor_x, y), Vec2::new(draw_width, draw_height));
                painter.image(
                    *texture_id,
                    rect,
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                cursor_x += draw_width;
            }
        }
    }
}

/// Paints one run of plain text at `pos` (its top-left corner) and returns its size.
/// `bold` repaints it `FAUX_BOLD_PASSES` more times, each shifted right by one more pixel
/// — see this module's doc comment on why (no real bold font weight available).
fn paint_run(
    painter: &Painter,
    pos: Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    bold: bool,
) -> Vec2 {
    let galley = painter.layout_no_wrap(text.to_string(), font, color);
    let size = galley.size();
    let passes = if bold { FAUX_BOLD_PASSES } else { 0 };
    for i in 0..=passes {
        painter.galley(Pos2::new(pos.x + i as f32, pos.y), galley.clone(), color);
    }
    size
}
