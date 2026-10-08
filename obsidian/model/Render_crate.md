# Render crate

The render crate (`blueboard-render`) holds the wgpu rendering pipeline.

The render crate performs [[Tessellation]] of [[Bezier_curve]] strokes into GPU meshes.

The render crate performs [[Viewport_culling]] using the [[RTree_index]].

The render crate applies [[LOD_simplification]] at low zoom levels.

The render crate depends on [[Core_crate]].

The render crate renders the [[Command_line]]'s bar using `egui`/`egui-wgpu`, as a second
render pass layered on top of the board — but it does not own input or interaction;
[[App_crate]] still handles all keyboard/mouse input directly and just tells the render
crate what text to display each frame.

The render crate holds the [[Theme]] colors (board/grid/stroke) as a small uniform buffer,
set once at startup from [[App_crate]]'s loaded [[Config_file]] — no live theme reload yet.

The command bar is now drawn every frame regardless of state (composing/status/idle),
with the active tool's name permanently shown on its left — a persistent status line, not
something that only appears on demand. While composing, it also renders the
[[Command_line]]'s live Tab-completion candidates as an extra row above the input.

Committed strokes/shapes and their live preview render through a per-vertex-colored path
(`ColorVertex`/`vs_colored`/`fs_colored`) carrying each element's own [[Scene_graph]]
color; the grid and UI-only overlays (selection boxes, the eraser cursor) stayed on the
original uniform theme-colored path, deliberately — they aren't board content and
shouldn't track whatever color the user's currently drawing with.

The reference grid is regenerated fresh every frame, sized to exactly the camera's
current visible extent, instead of a fixed buffer built once at startup — what makes it
look infinite at any zoom/pan. `:grid <show|hide|bigger|smaller|number>` controls it via
`GridSettings` on `App`, runtime-only (not persisted to [[Config_file]]).

`:color picker` opens the one genuinely interactive piece of UI this crate owns — an
egui color picker (saturation/value square + hue strip) — contradicting this note's
earlier "doesn't own input or interaction" claim in that one case: real pointer events
are forwarded to it (`Renderer::push_egui_event`) only while it's open, and
`blueboard-app` suppresses the active tool's own dispatch for the same duration.

`Renderer::render` takes one `Frame` struct now, not a positional argument list — grew
past clippy's `too_many_arguments` once grid settings and the color picker joined the
existing camera/scene/preview/command_bar/tool-name/scale_factor params.

