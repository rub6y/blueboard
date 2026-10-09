//! Paints a [`MathToken`] sequence (`blueboard_core::text_markup`) with `egui`'s painter:
//! bold via a faux-bold double-paint (this app's fonts have no separate bold weight),
//! superscript/subscript via a smaller font raised/lowered relative to the baseline, and
//! `\frac{a}{b}` as a real stacked fraction (numerator over denominator with a divider
//! line) — the one piece that can't be expressed as a single `egui::text::LayoutJob`
//! paragraph, which is why this is a hand-rolled left-to-right layout instead of one.
//! Deliberately single-line: no wrapping, matching `text_markup::parse`'s own scope.

use blueboard_core::text_markup::MathToken;
use egui::{Color32, FontId, Painter, Pos2, Vec2};

/// Relative to `font_size`: how much smaller a superscript/subscript or a fraction's
/// numerator/denominator render, how far a superscript/subscript shifts off the baseline,
/// and how much horizontal breathing room a fraction gets on each side.
const SCRIPT_SCALE: f32 = 0.65;
const SCRIPT_RAISE: f32 = 0.35;
const FRAC_SCALE: f32 = 0.8;
const FRAC_PADDING: f32 = 0.25;
/// Gap (both sides) between a fraction's numerator/denominator and its divider line.
const FRAC_LINE_GAP: f32 = 0.08;
/// How many extra times a faux-bold run is painted, each offset by one more pixel —
/// see this module's doc comment.
const FAUX_BOLD_PASSES: i32 = 2;

/// Paints `tokens` left-to-right starting at `top_left` (the text element's board-space
/// top-left corner, already converted to screen/logical points by the caller) and returns
/// the painted bounding size — callers don't need to also call
/// `text_markup::estimate_size`; this is the real painted size at this `font_size`.
pub fn paint_tokens(
    painter: &Painter,
    top_left: Pos2,
    tokens: &[MathToken],
    font_size: f32,
    color: Color32,
) -> Vec2 {
    let baseline_y = top_left.y + font_size * 1.0;
    let mut cursor_x = top_left.x;
    let mut min_y = top_left.y;
    let mut max_y = top_left.y + font_size * 1.3;

    for token in tokens {
        match token {
            MathToken::Text { text, bold } => {
                let font = FontId::proportional(font_size);
                let size = paint_run(
                    painter,
                    pos(cursor_x, baseline_y - font_size),
                    text,
                    font,
                    color,
                    *bold,
                );
                cursor_x += size.x;
            }
            MathToken::Superscript(text) => {
                let font = FontId::proportional(font_size * SCRIPT_SCALE);
                let y = baseline_y - font_size - font_size * SCRIPT_RAISE;
                min_y = min_y.min(y);
                let size = paint_run(painter, pos(cursor_x, y), text, font, color, false);
                cursor_x += size.x;
            }
            MathToken::Subscript(text) => {
                let font = FontId::proportional(font_size * SCRIPT_SCALE);
                let y = baseline_y - font_size + font_size * SCRIPT_RAISE;
                max_y = max_y.max(y + font_size * SCRIPT_SCALE * 1.3);
                let size = paint_run(painter, pos(cursor_x, y), text, font, color, false);
                cursor_x += size.x;
            }
            MathToken::Fraction(num, den) => {
                let frac_font = FontId::proportional(font_size * FRAC_SCALE);
                let padding = font_size * FRAC_PADDING;
                let num_size = measure(painter, num, frac_font.clone());
                let den_size = measure(painter, den, frac_font.clone());
                let column_width = num_size.x.max(den_size.x) + padding * 2.0;
                let line_y = baseline_y - font_size * 0.45;

                let num_x = cursor_x + (column_width - num_size.x) * 0.5;
                let num_y = line_y - font_size * FRAC_LINE_GAP - num_size.y;
                paint_run(
                    painter,
                    pos(num_x, num_y),
                    num,
                    frac_font.clone(),
                    color,
                    false,
                );

                let den_x = cursor_x + (column_width - den_size.x) * 0.5;
                let den_y = line_y + font_size * FRAC_LINE_GAP;
                paint_run(painter, pos(den_x, den_y), den, frac_font, color, false);

                painter.line_segment(
                    [pos(cursor_x, line_y), pos(cursor_x + column_width, line_y)],
                    egui::Stroke::new(1.0, color),
                );

                min_y = min_y.min(num_y);
                max_y = max_y.max(den_y + den_size.y);
                cursor_x += column_width;
            }
        }
    }

    Vec2::new(cursor_x - top_left.x, max_y - min_y)
}

fn pos(x: f32, y: f32) -> Pos2 {
    Pos2::new(x, y)
}

fn measure(painter: &Painter, text: &str, font: FontId) -> Vec2 {
    painter
        .layout_no_wrap(text.to_string(), font, Color32::WHITE)
        .size()
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
