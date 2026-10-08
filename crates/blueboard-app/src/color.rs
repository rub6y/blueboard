//! Named-color palette and hex parsing for the `:color` command.

/// A small, memorable set of names — not exhaustive, just enough to avoid typing hex for
/// the common cases. Anything else goes through [`parse_hex`].
const NAMED_COLORS: &[(&str, [f32; 3])] = &[
    ("white", [1.0, 1.0, 1.0]),
    ("black", [0.0, 0.0, 0.0]),
    ("red", [0.9, 0.2, 0.2]),
    ("green", [0.2, 0.8, 0.3]),
    ("blue", [0.3, 0.5, 0.95]),
    ("yellow", [0.95, 0.85, 0.2]),
    ("orange", [0.95, 0.55, 0.15]),
    ("purple", [0.6, 0.3, 0.85]),
    ("cyan", [0.2, 0.8, 0.85]),
    ("magenta", [0.9, 0.3, 0.75]),
    ("gray", [0.6, 0.6, 0.6]),
    ("grey", [0.6, 0.6, 0.6]),
];

/// Parses a `:color` argument: a name from [`NAMED_COLORS`] (case-insensitive), or a
/// `#rrggbb`/`rrggbb` hex triplet. `None` if it's neither.
pub fn parse_color(input: &str) -> Option<[f32; 3]> {
    let trimmed = input.trim();
    NAMED_COLORS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(trimmed))
        .map(|&(_, color)| color)
        .or_else(|| parse_hex(trimmed))
}

fn parse_hex(input: &str) -> Option<[f32; 3]> {
    let hex = input.strip_prefix('#').unwrap_or(input);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16)
            .ok()
            .map(|v| v as f32 / 255.0)
    };
    Some([channel(0..2)?, channel(2..4)?, channel(4..6)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_color_lookup_is_case_insensitive() {
        assert_eq!(parse_color("Red"), parse_color("red"));
        assert!(parse_color("red").is_some());
    }

    #[test]
    fn hex_parses_with_and_without_hash() {
        assert_eq!(parse_color("#ff0000"), parse_color("ff0000"));
        assert_eq!(parse_color("#ff8000"), Some([1.0, 0.5019608, 0.0]));
    }

    #[test]
    fn invalid_input_is_rejected() {
        assert_eq!(parse_color("notacolor"), None);
        assert_eq!(parse_color("#ff00"), None); // too short
        assert_eq!(parse_color("#gggggg"), None); // not hex digits
    }
}
