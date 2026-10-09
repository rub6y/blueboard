# Text tool

The text tool places text elements on the board: click to start composing (anchors the
element's top-left corner at that board point), type, Enter commits, Escape cancels.

Bound to `t` by default ([[Keybindings]]), selectable via `:tool text`
([[Command_registry]]) like any other [[Tool_registry]] entry — but its actual
interaction is special-cased in `main.rs`, not reached through the normal `Tool` trait
dispatch; see [[Tool_registry]]'s note on why.

Text content is lightweight markdown + a basic LaTeX-ish math subset, parsed by
`blueboard_core::text_markup` (pure, no rendering dependency, unit-tested standalone):
- `**bold**` toggles bold.
- `$$...$$` switches into math mode: `^`/`_` for superscript/subscript (`^x` or `^{xy}`),
  `\frac{a}{b}`, `\alpha`-style Greek letter commands, and `\mathbb{X}` (mapped to
  Unicode's mathematical double-struck block — `ℂ`/`ℍ`/`ℕ`/`ℙ`/`ℚ`/`ℝ`/`ℤ` are
  special-cased since Unicode gives those seven their own legacy letterlike-symbol
  codepoints instead of a slot in that block). **Known limit**: renders as a tofu box on
  a font that doesn't cover that Unicode block — confirmed against `egui`'s bundled
  font, which doesn't. Greek letters are unaffected (that block is covered).

Parsing produces a flat `Vec<MathToken>` (`Text`/`Superscript`/`Subscript`/`Fraction`),
painted left-to-right by `blueboard-render`'s `text` module — a hand-rolled layout, not
an `egui::text::LayoutJob`, specifically so `\frac{a}{b}` can be a *real* stacked
fraction (numerator over denominator, divider line) rather than an inline approximation;
bold fakes a heavier weight by repainting the glyphs offset by a pixel or two (no bold
font asset available).

A text element's bounding box (for hit-testing/the [[RTree_index]]) is estimated at
placement time by `text_markup::estimate_size` — a flat average character-advance ratio,
not real glyph metrics (those need `egui`'s font shaper, only reachable from
`blueboard-render`). Known limit: approximate, not pixel-exact — acceptable for
hit-test tolerance, not a precise click boundary.

See [[Scene_graph]] for how this introduced `Element`'s first real `kind` seam
(`ElementKind::Stroke` vs `ElementKind::Text`).
