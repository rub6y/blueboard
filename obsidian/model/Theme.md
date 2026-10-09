# Theme

The theme is the board/grid/stroke color set, configurable via the [[Config_file]].

The theme is loaded once at startup and passed into [[Render_crate]]'s `Renderer::new` as
`ThemeColors`, stored as a GPU uniform buffer — fixed for the process's lifetime, no live
reload yet.

The theme's `board` color is only the *initial* background value now — see
[[Background_customization]] for `:background`, which overrides it live (solid/gradient/
image) without touching the theme uniform itself.

The theme's default values are the colors chosen earlier in the project: deep blue board,
muted blue-grey grid, white strokes.

Since per-element color (see [[Scene_graph]]), the theme's `stroke` color only applies to
UI-only overlays (selection boxes, the eraser cursor) — board content uses whatever color
it was drawn in, set via `:color`, not this theme value. `DEFAULT_ELEMENT_COLOR`
(`blueboard-core`) happens to match the theme's old default (white), which is why nothing
visually changed for boards that never touch `:color`.
