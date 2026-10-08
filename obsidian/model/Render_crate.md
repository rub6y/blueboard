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

