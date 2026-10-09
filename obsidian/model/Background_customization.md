# Background customization

`:background` sets the board's background fill, owned by [[App_crate]] as
`BackgroundMode` (`blueboard-render`) and read fresh by [[Render_crate]] every frame:

- `:background color <hex>` — solid fill. Still just the render pass's `LoadOp::Clear`,
  recomputed from `BackgroundMode::Solid` each frame rather than a fixed field.
- `:background gradient <hex> <hex> <deg>` — a two-color gradient at any angle, drawn as a
  full-viewport quad through a dedicated pipeline (`vs_fullscreen`/`fs_background_gradient`
  in `shader.wgsl`) — a flat `LoadOp::Clear` can't express a gradient.
- `:background image <path> [zoom|tile]` — a PNG/JPEG (decoded via the `image` crate in
  `blueboard-app`, this crate's one dependency on it) uploaded once to the GPU via
  `Renderer::set_background_image`, not re-decoded every frame. Fit modes:
  - `zoom` (`ImageFit::ZoomWithCamera`): a board-space quad anchored at the origin, 1:1 to
    the image's native pixels — pans/zooms with the board like a watermark.
  - `tile` (`ImageFit::Tile`): a full-viewport quad, `uv` scaled by `viewport_px /
    image_px`, tiled via the sampler's repeat address mode — screen-locked, not
    board-anchored.

Companion feature, same command-line style: `:grid style <lines|horizontal|hex|dots>`
(`GridStyle` on [[Camera]]'s sibling `GridSettings`) — all four reuse the existing
line-list grid pipeline (a dot is a small cross, a hex cell is six line segments), no new
pipeline needed there, unlike background's gradient/image.

See the ledger's 2026-10-09 entry for implementation notes, including the WGSL
reserved-keyword bug (`from`/`to` as struct field names) caught via the wgpu
uncaptured-error log rather than the compiler.
