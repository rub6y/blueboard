//! Parses a text element's raw source into [`MathToken`]s: `**bold**` markdown, and
//! `$$...$$` math spans handed whole to a real LaTeX engine (`blueboard-app`'s `latex`
//! module — see its doc comment) rather than hand-parsed here. An earlier version of this
//! module reimplemented a small math subset itself (superscript/subscript/`\frac`/Greek
//! letters/`\mathbb`) with a hand-rolled painter in `blueboard-render`; that's gone now
//! that real LaTeX does all of it, more correctly, for free — a math span is just an
//! opaque source string from this crate's point of view. Kept here (not
//! `blueboard-render`/`blueboard-app`) so splitting markdown from math stays pure and
//! headlessly testable, per this crate's own "stays independent of the GPU/rendering
//! stack" convention (see `camera.rs`'s doc comment).

use serde::{Deserialize, Serialize};

/// One piece of a text element's parsed content, painted left-to-right by
/// `blueboard-render` (no line wrapping — plain prose doesn't need it, and a `Math` span
/// is rendered as a single image regardless of its content's shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MathToken {
    Text {
        text: String,
        bold: bool,
    },
    /// Raw LaTeX math source from between a `$$...$$` pair, untouched — real LaTeX
    /// (`blueboard-app::latex`) renders it to an image, keyed by this exact string.
    Math(String),
}

/// Parses `input`: `**...**` toggles bold in the surrounding markdown text, `$$...$$`
/// is captured whole as a [`MathToken::Math`].
pub fn parse(input: &str) -> Vec<MathToken> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut bold = false;
    let mut buf = String::new();

    while let Some(&c) = chars.peek() {
        match c {
            '*' => {
                chars.next();
                if chars.peek() == Some(&'*') {
                    chars.next();
                    flush_text(&mut tokens, &mut buf, bold);
                    bold = !bold;
                } else {
                    buf.push('*');
                }
            }
            '$' => {
                chars.next();
                if chars.peek() == Some(&'$') {
                    chars.next();
                    flush_text(&mut tokens, &mut buf, bold);
                    let math = read_until_double_dollar(&mut chars);
                    tokens.push(MathToken::Math(math));
                } else {
                    buf.push('$');
                }
            }
            _ => {
                buf.push(c);
                chars.next();
            }
        }
    }
    flush_text(&mut tokens, &mut buf, bold);
    tokens
}

fn flush_text(tokens: &mut Vec<MathToken>, buf: &mut String, bold: bool) {
    if !buf.is_empty() {
        tokens.push(MathToken::Text {
            text: std::mem::take(buf),
            bold,
        });
    }
}

fn read_until_double_dollar(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut math = String::new();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'$') {
            chars.next();
            break;
        }
        math.push(c);
    }
    math
}

/// Rough width/height (same units as `font_size`) for painting `tokens` in one line — used
/// to size a text element's board-space bounding box at placement time, independent of the
/// camera's current zoom and before any `Math` span has actually been rendered (that
/// happens later, asynchronously from this crate's point of view — see
/// `blueboard-app::latex`). Not real metrics: a `Text` run uses a flat average
/// character-advance ratio (no font shaper available here — see `camera.rs`'s doc
/// comment); a `Math` span just guesses from its source length, since its real rendered
/// size depends on real LaTeX output this crate never sees. Known limit: both are
/// estimates good enough for hit-testing/selection tolerance, not a pixel-exact bounding
/// box — a `Math` span's estimate in particular can be noticeably off once the real image
/// renders.
pub fn estimate_size(tokens: &[MathToken], font_size: f32) -> (f32, f32) {
    const CHAR_WIDTH_RATIO: f32 = 0.55;
    const LINE_HEIGHT_RATIO: f32 = 1.3;
    const MATH_CHAR_WIDTH_RATIO: f32 = 0.5;

    if tokens.is_empty() {
        return (font_size * CHAR_WIDTH_RATIO, font_size * LINE_HEIGHT_RATIO);
    }

    let mut width = 0.0f32;
    let mut height = font_size * LINE_HEIGHT_RATIO;
    for token in tokens {
        match token {
            MathToken::Text { text, .. } => {
                width += text.chars().count() as f32 * font_size * CHAR_WIDTH_RATIO;
            }
            MathToken::Math(source) => {
                width += source.chars().count().max(1) as f32 * font_size * MATH_CHAR_WIDTH_RATIO;
                height = height.max(font_size * LINE_HEIGHT_RATIO * 1.6);
            }
        }
    }
    (width.max(font_size * CHAR_WIDTH_RATIO), height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_token() {
        assert_eq!(
            parse("hello"),
            vec![MathToken::Text {
                text: "hello".to_string(),
                bold: false
            }]
        );
    }

    #[test]
    fn bold_toggles_around_double_asterisks() {
        assert_eq!(
            parse("**bold** plain"),
            vec![
                MathToken::Text {
                    text: "bold".to_string(),
                    bold: true
                },
                MathToken::Text {
                    text: " plain".to_string(),
                    bold: false
                },
            ]
        );
    }

    #[test]
    fn math_span_is_captured_whole_untouched() {
        assert_eq!(
            parse(r"$$\frac{1}{2} + \mathbb{R} \alpha$$"),
            vec![MathToken::Math(
                r"\frac{1}{2} + \mathbb{R} \alpha".to_string()
            )]
        );
    }

    #[test]
    fn mixed_text_and_math() {
        assert_eq!(
            parse(r"Energy: $$E = mc^2$$ done"),
            vec![
                MathToken::Text {
                    text: "Energy: ".to_string(),
                    bold: false
                },
                MathToken::Math("E = mc^2".to_string()),
                MathToken::Text {
                    text: " done".to_string(),
                    bold: false
                },
            ]
        );
    }

    #[test]
    fn estimate_size_grows_with_content_length_and_font_size() {
        let short = estimate_size(&parse("hi"), 20.0);
        let long = estimate_size(&parse("hello world"), 20.0);
        assert!(long.0 > short.0);

        let small = estimate_size(&parse("hello"), 10.0);
        let big = estimate_size(&parse("hello"), 20.0);
        assert!(big.0 > small.0);
        assert!(big.1 > small.1);
    }

    #[test]
    fn math_estimate_is_taller_than_plain_text() {
        let plain = estimate_size(&parse("x"), 20.0);
        let math = estimate_size(&parse(r"$$\frac{1}{2}$$"), 20.0);
        assert!(math.1 > plain.1);
    }
}
