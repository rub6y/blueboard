# Theme

The theme is the board/grid/stroke color set, configurable via the [[Config_file]].

The theme is loaded once at startup and passed into [[Render_crate]]'s `Renderer::new` as
`ThemeColors`, stored as a GPU uniform buffer — fixed for the process's lifetime, no live
reload yet.

The theme's default values are the colors chosen earlier in the project: deep blue board,
muted blue-grey grid, white strokes.
