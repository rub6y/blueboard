//! Parses a text element's raw source into [`MathToken`]s: `**bold**` markdown, and a
//! basic LaTeX-ish math subset inside `$$...$$` — superscript (`^`), subscript (`_`),
//! `\frac{a}{b}`, Greek letter commands (`\alpha`..), and `\mathbb{X}` (blackboard bold,
//! via Unicode's mathematical double-struck codepoints — no custom glyph rendering
//! needed). Deliberately not a real TeX engine: no nested math inside a fraction's
//! numerator/denominator, no line breaks, no nested braces beyond one level. Kept here
//! (not `blueboard-render`) so it's pure and headlessly testable, per this crate's own
//! "stays independent of the GPU/rendering stack" convention (see `camera.rs`'s doc
//! comment) — `blueboard-render` paints a `&[MathToken]` sequence, it doesn't parse one.

use serde::{Deserialize, Serialize};

/// One piece of a text element's parsed content, painted left-to-right by
/// `blueboard-render` (no line wrapping — see this module's doc comment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MathToken {
    Text {
        text: String,
        bold: bool,
    },
    Superscript(String),
    Subscript(String),
    /// `\frac{numerator}{denominator}` — plain text on each side, not recursive.
    Fraction(String, String),
}

/// Parses `input`: `**...**` toggles bold in the surrounding markdown text, `$$...$$`
/// switches into the math subset (see this module's doc comment) for its contents.
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
                    tokens.extend(parse_math(&math, bold));
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

/// A `{...}` group (returns its contents, braces stripped) if `chars` is positioned right
/// after a command name and the next char is `{`; otherwise the single next character, as
/// a one-character string — `^2` and `^{2}` both mean "superscript 2".
fn read_group_or_char(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    match chars.peek() {
        Some('{') => read_group(chars).unwrap_or_default(),
        Some(_) => chars.next().map(String::from).unwrap_or_default(),
        None => String::new(),
    }
}

/// Reads a `{...}` group starting at the current position, consuming through the closing
/// `}`. Returns `None` (consuming nothing) if the next char isn't `{`. No nested-brace
/// support — a `{` inside the group ends the group's content, not a nested one, per this
/// module's "not a real TeX engine" scope.
fn read_group(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    if chars.peek() != Some(&'{') {
        return None;
    }
    chars.next();
    let mut content = String::new();
    for c in chars.by_ref() {
        if c == '}' {
            return Some(content);
        }
        content.push(c);
    }
    Some(content)
}

fn read_command_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_alphabetic() {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    name
}

fn parse_math(math: &str, bold: bool) -> Vec<MathToken> {
    let mut tokens = Vec::new();
    let mut chars = math.chars().peekable();
    let mut buf = String::new();

    while let Some(&c) = chars.peek() {
        match c {
            '^' => {
                chars.next();
                let content = read_group_or_char(&mut chars);
                flush_text(&mut tokens, &mut buf, bold);
                tokens.push(MathToken::Superscript(content));
            }
            '_' => {
                chars.next();
                let content = read_group_or_char(&mut chars);
                flush_text(&mut tokens, &mut buf, bold);
                tokens.push(MathToken::Subscript(content));
            }
            '\\' => {
                chars.next();
                let name = read_command_name(&mut chars);
                match name.as_str() {
                    "frac" => {
                        let num = read_group(&mut chars).unwrap_or_default();
                        let den = read_group(&mut chars).unwrap_or_default();
                        flush_text(&mut tokens, &mut buf, bold);
                        tokens.push(MathToken::Fraction(num, den));
                    }
                    "mathbb" => {
                        let inner = read_group(&mut chars).unwrap_or_default();
                        buf.extend(inner.chars().map(mathbb_char));
                    }
                    "" => buf.push('\\'), // a lone backslash with no command name after it
                    other => match greek_symbol(other) {
                        Some(symbol) => buf.push(symbol),
                        // Unknown command: degrade gracefully to its literal source rather
                        // than silently dropping it.
                        None => {
                            buf.push('\\');
                            buf.push_str(other);
                        }
                    },
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

/// `\mathbb{X}`'s per-character mapping to Unicode's mathematical double-struck block.
/// Seven capital letters (C, H, N, P, Q, R, Z) are a deliberate hole in that block —
/// Unicode instead gives those their own long-established "letterlike symbol" codepoints
/// (ℂ, ℍ, ℕ, ℙ, ℚ, ℝ, ℤ), so they're special-cased rather than computed from an offset.
fn mathbb_char(c: char) -> char {
    match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        'A'..='Z' => char::from_u32(0x1D538 + (c as u32 - 'A' as u32)).unwrap_or(c),
        'a'..='z' => char::from_u32(0x1D552 + (c as u32 - 'a' as u32)).unwrap_or(c),
        other => other,
    }
}

/// Lowercase and the handful of visually-distinct uppercase Greek letter commands — the
/// uppercase letters that just look like their Latin counterpart (`\Alpha`, `\Beta`, ...)
/// aren't real LaTeX commands either, so they're not included here.
fn greek_symbol(name: &str) -> Option<char> {
    Some(match name {
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "epsilon" => 'ε',
        "zeta" => 'ζ',
        "eta" => 'η',
        "theta" => 'θ',
        "iota" => 'ι',
        "kappa" => 'κ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "nu" => 'ν',
        "xi" => 'ξ',
        "pi" => 'π',
        "rho" => 'ρ',
        "sigma" => 'σ',
        "tau" => 'τ',
        "upsilon" => 'υ',
        "phi" => 'φ',
        "chi" => 'χ',
        "psi" => 'ψ',
        "omega" => 'ω',
        "Gamma" => 'Γ',
        "Delta" => 'Δ',
        "Theta" => 'Θ',
        "Lambda" => 'Λ',
        "Xi" => 'Ξ',
        "Pi" => 'Π',
        "Sigma" => 'Σ',
        "Upsilon" => 'Υ',
        "Phi" => 'Φ',
        "Psi" => 'Ψ',
        "Omega" => 'Ω',
        _ => return None,
    })
}

/// Rough width/height (same units as `font_size`) for painting `tokens` in one line — used
/// to size a text element's board-space bounding box at placement time, independent of the
/// camera's current zoom. Not real glyph metrics (that needs a font shaper, only available
/// in `blueboard-render` via `egui`'s fonts at actual paint time) — a flat average
/// character-advance ratio. Known limit: a proportional font's real per-glyph widths vary
/// more than this assumes, so the box is an estimate good enough for hit-testing/selection
/// tolerance, not a pixel-exact click boundary.
pub fn estimate_size(tokens: &[MathToken], font_size: f32) -> (f32, f32) {
    const CHAR_WIDTH_RATIO: f32 = 0.55;
    const LINE_HEIGHT_RATIO: f32 = 1.3;
    const SCRIPT_SCALE: f32 = 0.65;
    const FRAC_SCALE: f32 = 0.8;
    const FRAC_PADDING: f32 = 0.3;

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
            MathToken::Superscript(s) | MathToken::Subscript(s) => {
                width +=
                    s.chars().count().max(1) as f32 * font_size * SCRIPT_SCALE * CHAR_WIDTH_RATIO;
                height = height.max(font_size * LINE_HEIGHT_RATIO * 1.15);
            }
            MathToken::Fraction(num, den) => {
                let frac_chars = num.chars().count().max(den.chars().count()).max(1) as f32;
                width += frac_chars * font_size * FRAC_SCALE * CHAR_WIDTH_RATIO
                    + font_size * FRAC_PADDING;
                height = height.max(font_size * LINE_HEIGHT_RATIO * 2.0);
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
    fn math_superscript_and_subscript() {
        assert_eq!(
            parse("$$x^2_i$$"),
            vec![
                MathToken::Text {
                    text: "x".to_string(),
                    bold: false
                },
                MathToken::Superscript("2".to_string()),
                MathToken::Subscript("i".to_string()),
            ]
        );
    }

    #[test]
    fn braced_superscript_reads_the_whole_group() {
        assert_eq!(
            parse("$$x^{10}$$"),
            vec![
                MathToken::Text {
                    text: "x".to_string(),
                    bold: false
                },
                MathToken::Superscript("10".to_string()),
            ]
        );
    }

    #[test]
    fn fraction_splits_numerator_and_denominator() {
        assert_eq!(
            parse(r"$$\frac{1}{2}$$"),
            vec![MathToken::Fraction("1".to_string(), "2".to_string())]
        );
    }

    #[test]
    fn mathbb_maps_to_double_struck_unicode() {
        assert_eq!(
            parse(r"$$\mathbb{R}$$"),
            vec![MathToken::Text {
                text: "ℝ".to_string(),
                bold: false
            }]
        );
        assert_eq!(
            parse(r"$$\mathbb{A}$$"),
            vec![MathToken::Text {
                text: "𝔸".to_string(),
                bold: false
            }]
        );
    }

    #[test]
    fn greek_letter_commands_resolve_to_symbols() {
        assert_eq!(
            parse(r"$$\alpha + \beta$$"),
            vec![MathToken::Text {
                text: "α + β".to_string(),
                bold: false
            }]
        );
    }

    #[test]
    fn unknown_command_degrades_to_its_literal_source() {
        assert_eq!(
            parse(r"$$\notacommand$$"),
            vec![MathToken::Text {
                text: r"\notacommand".to_string(),
                bold: false
            }]
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
    fn fraction_estimate_is_taller_than_plain_text() {
        let plain = estimate_size(&parse("x"), 20.0);
        let frac = estimate_size(&parse(r"$$\frac{1}{2}$$"), 20.0);
        assert!(frac.1 > plain.1);
    }
}
