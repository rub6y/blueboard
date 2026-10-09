//! wgpu pipeline, tessellation + caching, viewport culling/LOD. See PLAN.md Phase 1-2.
//!
//! Phase 1 drew a static reference grid so camera pan/zoom had something to confirm
//! against. Phase 2 adds the real thing: pen strokes from `blueboard-core`'s scene graph,
//! tessellated by `lyon` into GPU triangles, queried each frame from the scene's R-tree by
//! viewport. Phase 4 adds a configurable color theme and the `:`-command bar, rendered
//! with `egui`/`egui-wgpu` layered on top of the board in a second pass. Phase 6 adds
//! per-element tessellation caching and reused/grown (rather than per-frame-reallocated)
//! GPU vertex buffers, once profiling against a large synthetic board (PLAN.md) showed
//! both actually mattered at real board sizes.

use std::sync::Arc;

use blueboard_core::camera::Camera;
use blueboard_core::scene::{ElementKind, Scene};
use kurbo::Shape as _;

mod text;
use glam::DVec2;
use lyon::math::point;
use lyon::path::Path as LyonPath;
use lyon::tessellation::{
    BuffersBuilder, StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use wgpu::util::DeviceExt;

/// Board-space width of a pen stroke. Fixed in board space (not screen space), so a
/// stroke scales with the board under zoom — like real ink, rather than staying a
/// constant number of screen pixels regardless of zoom level.
pub const STROKE_WIDTH: f32 = 2.5;

/// Stroke tessellation's flattening tolerance, in *screen-space* pixels — see
/// `planning/math.tex` §3: tolerance must be screen-space and divided by zoom before use,
/// so curve flattening tightens automatically as the user zooms in, instead of a fixed
/// board-space tolerance becoming visibly coarse (faceted) at high zoom.
const STROKE_TESSELLATION_TOLERANCE_PX: f32 = 0.3;

/// MSAA sample count for all board rendering. Without this, every triangle edge from
/// stroke tessellation is a hard, aliased boundary — this is what actually fixes the
/// "jagged/pixelated" look, which is a rasterization problem, not a curve-fitting one.
const SAMPLE_COUNT: u32 = 4;

/// The board/grid/stroke colors, loaded from `blueboard-app`'s config file and fixed for
/// the process's lifetime (no live theme reload yet — that would mean rebuilding the
/// theme uniform buffer on change, not a bigger change, just not wired up this phase).
#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub board: [f32; 3],
    pub grid: [f32; 3],
    pub stroke: [f32; 3],
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            board: [0.05, 0.08, 0.20],
            grid: [0.3, 0.38, 0.58],
            stroke: [1.0, 1.0, 1.0],
        }
    }
}

/// What to draw the reference grid as this frame — owned by `blueboard-app`'s `App` (the
/// `:grid` command mutates it directly) and passed into [`Renderer::render`] each call,
/// same as everything else about a frame. Board-space, same units as everything else
/// (`STROKE_WIDTH`, element paths, ...), not screen pixels.
#[derive(Debug, Clone, Copy)]
pub struct GridSettings {
    pub visible: bool,
    pub spacing: f32,
    pub style: GridStyle,
}

/// Which pure function (`generate_grid_lines` and its siblings) builds this frame's grid
/// vertices. All four draw through the same `grid_pipeline` (a uniform theme-colored
/// `LineList`) — a hex cell or a dot is approximated as short line segments rather than
/// needing a second grid pipeline/shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridStyle {
    #[default]
    Lines,
    HorizontalOnly,
    Hex,
    Dots,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            visible: true,
            spacing: 100.0,
            style: GridStyle::Lines,
        }
    }
}

/// How a background image fills the board — set with `:background image <path> <fit>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFit {
    /// Anchored at the board-space origin, sized 1:1 to the image's native pixels — it
    /// pans and zooms with the board like any other element, a fixed "watermark" rather
    /// than something that fills the whole canvas.
    ZoomWithCamera,
    /// Repeats across the entire viewport at native pixel size, screen-space (not
    /// board-space) — panning doesn't reveal new image content, the tile just stays
    /// screen-locked, like a desktop wallpaper.
    Tile,
}

/// What to fill the board's background with this frame — owned by `blueboard-app`'s `App`
/// (the `:background` command mutates it directly) and passed into [`Frame`] each call,
/// same as [`GridSettings`]. `Image`'s actual pixel data isn't here — it's uploaded once
/// via [`Renderer::set_background_image`] when the command runs, and lives in the
/// `Renderer` itself (re-decoding/re-uploading the image every frame would be wasteful);
/// this variant just selects which already-uploaded image's fit mode to draw with.
#[derive(Debug, Clone, Copy)]
pub enum BackgroundMode {
    Solid([f32; 3]),
    Gradient {
        from: [f32; 3],
        to: [f32; 3],
        /// Direction the gradient runs, degrees, 0 = left-to-right.
        angle_deg: f32,
    },
    Image(ImageFit),
}

impl Default for BackgroundMode {
    /// Just enough of a default to satisfy `App`'s `#[derive(Default)]` in
    /// `blueboard-app` — always immediately overridden there from the loaded theme's
    /// board color (same pattern as `current_color`'s own doc comment there), never
    /// meant to be seen.
    fn default() -> Self {
        Self::Solid([0.0, 0.0, 0.0])
    }
}

/// Mirrors `ThemeUniform` in `shader.wgsl`: two `vec4`s, not `vec3`s, since WGSL pads a
/// bare `vec3<f32>` to 16 bytes in a uniform block anyway — using `vec4` on both sides
/// keeps the Rust and WGSL layouts matching without relying on padding rules.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ThemeUniform {
    grid_color: [f32; 4],
    stroke_color: [f32; 4],
}

impl From<ThemeColors> for ThemeUniform {
    fn from(theme: ThemeColors) -> Self {
        let to_vec4 = |c: [f32; 3]| [c[0], c[1], c[2], 1.0];
        Self {
            grid_color: to_vec4(theme.grid),
            stroke_color: to_vec4(theme.stroke),
        }
    }
}

/// One command-line Tab-completion candidate: a command's name and usage hint, the same
/// pair `:help` lists — a small named struct rather than a bare tuple so the UI code
/// doesn't have to remember which element is which.
pub struct Completion {
    pub name: &'static str,
    pub usage: &'static str,
}

/// What the `:`-command bar should show this frame — built from `blueboard-app`'s input
/// state, analogous to [`ScenePreview`] for the active tool.
pub enum CommandBarContent<'a> {
    /// Not in command mode and no status to show.
    Hidden,
    /// Actively typing a command; `text` is everything after the `:`. `completions` is
    /// every registered command matching what's been typed so far (filtered live, not
    /// only after Tab — qutebrowser-style discoverability), and `selected` is which one
    /// Tab has cycled to, if any.
    Editing {
        text: &'a str,
        completions: &'a [Completion],
        selected: Option<usize>,
    },
    /// The result of the last command, shown until replaced by the next one.
    Status { message: &'a str, is_error: bool },
}

/// What the currently-active tool wants drawn as a live, not-yet-committed preview —
/// built from `blueboard-app`'s tool state each frame. Two shapes because the pen and
/// shape tools preview fundamentally differently: the pen shows its *raw*, not-yet-fitted
/// samples (so the user sees their actual hand motion while drawing), while a shape tool's
/// preview already *is* the exact path that will be committed, so it's rendered the same
/// way a committed element is.
pub enum ScenePreview<'a> {
    None,
    /// Raw, not-yet-curve-fitted board-space pointer samples, in the color they'll commit
    /// in — drawn as a thin line strip, deliberately distinct from how a committed stroke
    /// renders.
    RawPoints {
        points: &'a [DVec2],
        color: [f32; 3],
    },
    /// An exact path (e.g. a shape tool's in-progress rectangle/ellipse/line), in the
    /// color it'll commit in — drawn tessellated, identically to a committed element.
    Path {
        path: &'a kurbo::BezPath,
        color: [f32; 3],
    },
    /// The selection tool's selected-element indicator: a board-space bounding box, drawn
    /// as a plain outline (not tessellated like `Path`) so it reads as "this is marked,"
    /// not as content on the board.
    Selection(kurbo::Rect),
    /// Either eraser's cursor: a board-space circle outline showing what's about to be
    /// erased (and, for the partial eraser, exactly how big a bite the next erase will
    /// take) — same plain-outline treatment as `Selection`.
    EraseCursor {
        center: DVec2,
        radius: f64,
    },
    /// The rectangle/lasso multi-select tools' selected-group indicator: one bounding-box
    /// outline per selected element, same plain-outline treatment as `Selection` — owned
    /// rather than borrowed since these are recomputed fresh each frame (a group drag
    /// moves every box), not references into the tool's own stored state.
    MultiSelection(Vec<kurbo::Rect>),
}

/// One committed `Text` element, ready to paint — built fresh each frame in `render` from
/// whatever's currently visible (see `ElementKind::Text`), `runs` already parsed via
/// `text_markup::parse` and resolved against `Renderer::math_textures` (cheap for the
/// short strings a board's text elements realistically hold), and `screen_pos`/
/// `font_size_px` already converted out of board space — `text::paint_runs` doesn't need
/// the camera, or any `Renderer` state, at all.
struct SceneTextDraw {
    screen_pos: egui::Pos2,
    runs: Vec<text::TextRun>,
    font_size_px: f32,
    color: egui::Color32,
}

/// A text element still being composed (between a text-tool click and Enter/Escape) —
/// `blueboard-app`'s `App` owns the actual state (see its `text_input_*` fields), this is
/// just the read-only view passed in each frame so it paints live, same shape
/// `color_picker` on [`Frame`] already has for the color picker's own App-owned UI state.
pub struct TextInputPreview<'a> {
    pub board_pos: DVec2,
    pub content: &'a str,
    pub font_size: f32,
    pub color: [f32; 3],
}

/// Everything [`Renderer::render`] needs to draw one frame, bundled so the method takes
/// one argument instead of an ever-growing positional list.
pub struct Frame<'a> {
    pub camera: &'a Camera,
    pub scene: &'a Scene,
    pub preview: ScenePreview<'a>,
    pub command_bar: CommandBarContent<'a>,
    pub active_tool_name: &'a str,
    pub grid: GridSettings,
    pub background: BackgroundMode,
    pub text_input: Option<TextInputPreview<'a>>,
    /// `Some(color)` shows the color picker this frame, editing `color` in place; `None`
    /// keeps it closed. Only while open does real pointer/keyboard input reach egui at
    /// all — see `Renderer::egui_pending_events`.
    pub color_picker: Option<&'a mut [f32; 3]>,
    pub scale_factor: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Per-element colored content: committed strokes/shapes and their live preview — see
/// `shader.wgsl`'s `vs_colored`/`fs_colored`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ColorVertex {
    position: [f32; 2],
    color: [f32; 3],
}

impl ColorVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x3];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ColorVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// A background quad's vertex: position plus a texture coordinate — see
/// `shader.wgsl`'s `vs_fullscreen`/`vs_board_tex`/`fs_background_image` doc comment for
/// how the two background-image fit modes and the gradient both reuse this one shape.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TexVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

impl TexVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TexVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Mirrors `BgGradientUniform` in `shader.wgsl` — see that struct's doc comment.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BgGradientUniform {
    from_color: [f32; 4],
    to_color: [f32; 4],
    dir: [f32; 2],
    _pad: [f32; 2],
}

/// Six vertices (two triangles) covering the full clip-space viewport, `uv` spanning
/// `[0, uv_scale]` corner to corner — `uv_scale = [1, 1]` for the gradient (uv read
/// directly as a 0..1 screen position) and a larger scale for a tiled background image
/// (uv values beyond 1 wrap via `background_sampler`'s repeat address mode, which is what
/// actually produces the tiling). uv's Y matches this file's board/screen convention
/// (down is positive), opposite clip space's Y-up, hence the flip here.
fn fullscreen_quad_vertices(uv_scale: [f32; 2]) -> [TexVertex; 6] {
    let [su, sv] = uv_scale;
    let top_left = TexVertex {
        position: [-1.0, 1.0],
        uv: [0.0, 0.0],
    };
    let top_right = TexVertex {
        position: [1.0, 1.0],
        uv: [su, 0.0],
    };
    let bottom_left = TexVertex {
        position: [-1.0, -1.0],
        uv: [0.0, sv],
    };
    let bottom_right = TexVertex {
        position: [1.0, -1.0],
        uv: [su, sv],
    };
    [
        top_left,
        bottom_left,
        bottom_right,
        top_left,
        bottom_right,
        top_right,
    ]
}

/// A board-space quad anchored at the origin, sized 1:1 to `(image_width, image_height)`
/// pixels — `BackgroundMode::Image(ImageFit::ZoomWithCamera)`'s geometry, drawn through
/// `vs_board_tex` so it transforms with the camera like any other board content.
fn board_image_quad_vertices(image_width: f32, image_height: f32) -> [TexVertex; 6] {
    let top_left = TexVertex {
        position: [0.0, 0.0],
        uv: [0.0, 0.0],
    };
    let top_right = TexVertex {
        position: [image_width, 0.0],
        uv: [1.0, 0.0],
    };
    let bottom_left = TexVertex {
        position: [0.0, image_height],
        uv: [0.0, 1.0],
    };
    let bottom_right = TexVertex {
        position: [image_width, image_height],
        uv: [1.0, 1.0],
    };
    [
        top_left,
        bottom_left,
        bottom_right,
        top_left,
        bottom_right,
        top_right,
    ]
}

/// Hard cap on grid lines generated per axis, regardless of configured spacing or zoom —
/// without this, a very small `GridSettings::spacing` combined with being zoomed far out
/// (the visible board-space extent can be enormous at `MIN_ZOOM`) could generate an
/// unreasonable vertex count every single frame. The effective spacing widens past this
/// cap instead of refusing to draw, so the grid stays visible (just coarser) rather than
/// disappearing.
const MAX_GRID_LINES_PER_AXIS: f64 = 500.0;

/// The reference grid, covering exactly `[min, max]` (board-space) at `spacing` — called
/// fresh every frame with the camera's current visible extent (the same `board_min`/
/// `board_max` viewport culling already computes), rather than a fixed-size buffer built
/// once at startup, so the grid covers whatever's visible at any zoom/pan rather than
/// only ever back to the start. Lines are snapped to spacing-aligned world positions
/// (`floor`, not the raw `min`) so the grid doesn't visibly "swim" relative to board
/// content while panning between frames.
fn generate_grid_lines(min: DVec2, max: DVec2, spacing: f32) -> Vec<Vertex> {
    if spacing <= 0.0 {
        return Vec::new();
    }
    let width = (max.x - min.x).max(0.0);
    let height = (max.y - min.y).max(0.0);
    let spacing = (spacing as f64)
        .max(width / MAX_GRID_LINES_PER_AXIS)
        .max(height / MAX_GRID_LINES_PER_AXIS);

    let mut verts = Vec::new();

    let mut x = (min.x / spacing).floor() * spacing;
    while x <= max.x {
        verts.push(Vertex {
            position: [x as f32, min.y as f32],
        });
        verts.push(Vertex {
            position: [x as f32, max.y as f32],
        });
        x += spacing;
    }

    let mut y = (min.y / spacing).floor() * spacing;
    while y <= max.y {
        verts.push(Vertex {
            position: [min.x as f32, y as f32],
        });
        verts.push(Vertex {
            position: [max.x as f32, y as f32],
        });
        y += spacing;
    }

    verts
}

/// Same idea as [`generate_grid_lines`] but only the horizontal pass — a "lined paper"
/// variant.
fn generate_grid_horizontal(min: DVec2, max: DVec2, spacing: f32) -> Vec<Vertex> {
    if spacing <= 0.0 {
        return Vec::new();
    }
    let width = (max.x - min.x).max(0.0);
    let height = (max.y - min.y).max(0.0);
    let spacing = (spacing as f64)
        .max(width / MAX_GRID_LINES_PER_AXIS)
        .max(height / MAX_GRID_LINES_PER_AXIS);

    let mut verts = Vec::new();
    let mut y = (min.y / spacing).floor() * spacing;
    while y <= max.y {
        verts.push(Vertex {
            position: [min.x as f32, y as f32],
        });
        verts.push(Vertex {
            position: [max.x as f32, y as f32],
        });
        y += spacing;
    }
    verts
}

/// Hard cap on grid cells generated per axis for the two-dimensional grid styles (hex,
/// dots) — much smaller than [`MAX_GRID_LINES_PER_AXIS`] since those generate O(rows *
/// cols) geometry rather than O(rows + cols) for a line grid, at the same zoomed-out
/// extent.
const MAX_GRID_POINTS_PER_AXIS: f64 = 150.0;

/// A dot grid: a small "+" cross (two short line segments) at each grid intersection,
/// rather than a filled circle — the grid only has a `LineList` pipeline to draw through
/// (see `GridStyle`'s doc comment), and a cross reads clearly as "a dot" at the sizes this
/// renders at.
fn generate_grid_dots(min: DVec2, max: DVec2, spacing: f32) -> Vec<Vertex> {
    if spacing <= 0.0 {
        return Vec::new();
    }
    let width = (max.x - min.x).max(0.0);
    let height = (max.y - min.y).max(0.0);
    let spacing = (spacing as f64)
        .max(width / MAX_GRID_POINTS_PER_AXIS)
        .max(height / MAX_GRID_POINTS_PER_AXIS);
    let mark_radius = spacing * 0.08;

    let mut verts = Vec::new();
    let mut y = (min.y / spacing).floor() * spacing;
    while y <= max.y {
        let mut x = (min.x / spacing).floor() * spacing;
        while x <= max.x {
            verts.push(Vertex {
                position: [(x - mark_radius) as f32, y as f32],
            });
            verts.push(Vertex {
                position: [(x + mark_radius) as f32, y as f32],
            });
            verts.push(Vertex {
                position: [x as f32, (y - mark_radius) as f32],
            });
            verts.push(Vertex {
                position: [x as f32, (y + mark_radius) as f32],
            });
            x += spacing;
        }
        y += spacing;
    }
    verts
}

/// A pointy-top hex grid, each cell's six edges drawn as line segments (shared edges
/// between neighbors are drawn twice — harmless overdraw under this pipeline's `REPLACE`
/// blend, simpler than deduplicating). `spacing` is treated as a hex's circumradius * 2,
/// so it reads on the same scale the other grid styles' "cell size" does.
fn generate_grid_hex(min: DVec2, max: DVec2, spacing: f32) -> Vec<Vertex> {
    if spacing <= 0.0 {
        return Vec::new();
    }
    let width = (max.x - min.x).max(0.0);
    let height = (max.y - min.y).max(0.0);
    let radius = (spacing as f64 / 2.0)
        .max(width / MAX_GRID_POINTS_PER_AXIS)
        .max(height / MAX_GRID_POINTS_PER_AXIS);
    let hex_width = radius * 3f64.sqrt();
    let row_height = radius * 1.5;

    let mut verts = Vec::new();
    let row_start = (min.y / row_height).floor() as i64 - 1;
    let row_end = (max.y / row_height).ceil() as i64 + 1;
    for row in row_start..=row_end {
        let center_y = row as f64 * row_height;
        let row_offset = if row % 2 != 0 { hex_width / 2.0 } else { 0.0 };
        let col_start = ((min.x - row_offset) / hex_width).floor() as i64 - 1;
        let col_end = ((max.x - row_offset) / hex_width).ceil() as i64 + 1;
        for col in col_start..=col_end {
            let center_x = col as f64 * hex_width + row_offset;
            let mut corners = [DVec2::ZERO; 6];
            for (k, corner) in corners.iter_mut().enumerate() {
                let angle = std::f64::consts::FRAC_PI_2 + (k as f64) * std::f64::consts::FRAC_PI_3;
                *corner = DVec2::new(
                    center_x + radius * angle.cos(),
                    center_y + radius * angle.sin(),
                );
            }
            for k in 0..6 {
                let a = corners[k];
                let b = corners[(k + 1) % 6];
                verts.push(Vertex {
                    position: [a.x as f32, a.y as f32],
                });
                verts.push(Vertex {
                    position: [b.x as f32, b.y as f32],
                });
            }
        }
    }
    verts
}

/// Convert a kurbo path (the scene graph's stroke representation) into a lyon path (what
/// the tessellator consumes). The two crates don't share a path type, so this walks
/// kurbo's path elements and replays them on a lyon path builder.
fn kurbo_path_to_lyon(path: &kurbo::BezPath) -> LyonPath {
    let mut builder = LyonPath::builder();
    let mut closed = false;

    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) => {
                builder.begin(point(p.x as f32, p.y as f32));
            }
            kurbo::PathEl::LineTo(p) => {
                builder.line_to(point(p.x as f32, p.y as f32));
            }
            kurbo::PathEl::QuadTo(c, p) => {
                builder.quadratic_bezier_to(
                    point(c.x as f32, c.y as f32),
                    point(p.x as f32, p.y as f32),
                );
            }
            kurbo::PathEl::CurveTo(c1, c2, p) => {
                builder.cubic_bezier_to(
                    point(c1.x as f32, c1.y as f32),
                    point(c2.x as f32, c2.y as f32),
                    point(p.x as f32, p.y as f32),
                );
            }
            kurbo::PathEl::ClosePath => {
                builder.end(true);
                closed = true;
            }
        }
    }

    if !closed {
        builder.end(false);
    }
    builder.build()
}

/// Tessellate one stroke's outline into triangles, in `color`, appending them (as a flat,
/// non-indexed vertex list) to `out`. `tolerance` is in the same board-space units as
/// `path` — callers are responsible for converting a screen-space pixel tolerance by
/// dividing by zoom (see [`STROKE_TESSELLATION_TOLERANCE_PX`]).
fn tessellate_colored_stroke(
    path: &kurbo::BezPath,
    color: [f32; 3],
    width: f32,
    tolerance: f32,
    out: &mut Vec<ColorVertex>,
) {
    let lyon_path = kurbo_path_to_lyon(path);
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_tolerance(tolerance)
        .with_line_join(lyon::tessellation::LineJoin::Round)
        .with_start_cap(lyon::tessellation::LineCap::Round)
        .with_end_cap(lyon::tessellation::LineCap::Round);

    let mut buffers: VertexBuffers<ColorVertex, u32> = VertexBuffers::new();
    let mut tessellator = StrokeTessellator::new();
    let result = tessellator.tessellate_path(
        &lyon_path,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |vertex: StrokeVertex| ColorVertex {
            position: vertex.position().to_array(),
            color,
        }),
    );

    // A malformed path (e.g. a degenerate curve from an edge case in curve fitting)
    // should drop just this one stroke, logged, rather than panicking the whole app —
    // losing one stroke's render is recoverable, a hard crash mid-drawing isn't.
    if let Err(err) = result {
        log::error!("stroke tessellation failed, skipping this stroke: {err:?}");
        return;
    }

    out.extend(
        buffers
            .indices
            .iter()
            .map(|&index| buffers.vertices[index as usize]),
    );
}

/// Build a single live-preview line strip (as a `LineList`, one segment per consecutive
/// pair), in `color`, from the raw, not-yet-fitted points of a stroke still being drawn.
fn colored_preview_line_vertices(points: &[DVec2], color: [f32; 3]) -> Vec<ColorVertex> {
    points
        .windows(2)
        .flat_map(|pair| {
            [
                ColorVertex {
                    position: [pair[0].x as f32, pair[0].y as f32],
                    color,
                },
                ColorVertex {
                    position: [pair[1].x as f32, pair[1].y as f32],
                    color,
                },
            ]
        })
        .collect()
}

/// Build a closed rectangle outline (four `LineList` segments) for `ScenePreview::Selection`.
fn selection_rect_vertices(rect: kurbo::Rect) -> Vec<Vertex> {
    let corners = [
        [rect.x0 as f32, rect.y0 as f32],
        [rect.x1 as f32, rect.y0 as f32],
        [rect.x1 as f32, rect.y1 as f32],
        [rect.x0 as f32, rect.y1 as f32],
    ];
    (0..4)
        .flat_map(|i| {
            [
                Vertex {
                    position: corners[i],
                },
                Vertex {
                    position: corners[(i + 1) % 4],
                },
            ]
        })
        .collect()
}

/// Number of segments approximating the eraser cursor's circle — plenty for something
/// drawn at cursor size; no need for the zoom-dependent tessellation tolerance the actual
/// board content uses.
const ERASE_CURSOR_SEGMENTS: u32 = 32;

/// Build a circle outline (as `LineList` segments) for `ScenePreview::EraseCursor`.
fn erase_cursor_vertices(center: DVec2, radius: f64) -> Vec<Vertex> {
    let point_at = |i: u32| {
        let angle = (i as f64 / ERASE_CURSOR_SEGMENTS as f64) * std::f64::consts::TAU;
        Vertex {
            position: [
                (center.x + radius * angle.cos()) as f32,
                (center.y + radius * angle.sin()) as f32,
            ],
        }
    };
    (0..ERASE_CURSOR_SEGMENTS)
        .flat_map(|i| [point_at(i), point_at(i + 1)])
        .collect()
}

fn color_to_color32(color: [f32; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn color32_to_color(color: egui::Color32) -> [f32; 3] {
    [
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
    ]
}

/// The offscreen multisampled color target every pipeline renders into; `render()`
/// resolves it down into the swapchain's single-sample texture each frame.
fn create_msaa_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("msaa color target"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: SAMPLE_COUNT,
        dimension: wgpu::TextureDimension::D2,
        format: config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Bar height with no completion row showing.
const COMMAND_BAR_HEIGHT: f32 = 24.0;
/// Extra height given to the completion row, shown above the input line while a typed
/// command prefix matches at least one registered command.
const COMPLETION_ROW_HEIGHT: f32 = 20.0;

/// Lay out the command bar as a thin bottom panel and return what egui produced —
/// shapes to tessellate and any texture updates (the font atlas, the first time text is
/// drawn). Pure UI description; no wgpu here.
///
/// Always drawn, even with `content: Hidden` — `active_tool_name` on its left makes the
/// bar a persistent status line (which tool is active), the same idea as vim's own bottom
/// status line always being present, not just when a command is being typed.
/// Bundles `run_command_bar_ui`'s read-only per-frame UI content (as opposed to the
/// input/state params it also takes) — otherwise clippy's `too_many_arguments` kicks in,
/// same reasoning as [`Frame`] bundling `Renderer::render`'s own growing argument list.
struct CommandBarInput<'a> {
    content: &'a CommandBarContent<'a>,
    active_tool_name: &'a str,
    text_draws: &'a [SceneTextDraw],
}

fn run_command_bar_ui(
    ctx: &egui::Context,
    viewport_size: (u32, u32),
    scale_factor: f32,
    input: CommandBarInput<'_>,
    events: Vec<egui::Event>,
    mut color_picker: Option<&mut [f32; 3]>,
) -> egui::FullOutput {
    let CommandBarInput {
        content,
        active_tool_name,
        text_draws,
    } = input;
    let screen_size_points = egui::vec2(
        viewport_size.0 as f32 / scale_factor,
        viewport_size.1 as f32 / scale_factor,
    );
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            screen_size_points,
        )),
        events,
        ..Default::default()
    };

    let completions = match content {
        CommandBarContent::Editing { completions, .. } if !completions.is_empty() => {
            Some(*completions)
        }
        _ => None,
    };
    let bar_height = COMMAND_BAR_HEIGHT + completions.map_or(0.0, |_| COMPLETION_ROW_HEIGHT);

    ctx.run_ui(raw_input, |ui| {
        // Scene text painted before the panels below, directly on the background layer —
        // see `SceneTextDraw`'s doc comment for why this data arrives pre-converted to
        // screen space/logical points rather than needing the camera here.
        let painter = ui.painter();
        for draw in text_draws {
            text::paint_runs(
                painter,
                draw.screen_pos,
                &draw.runs,
                draw.font_size_px,
                draw.color,
            );
        }

        // egui 0.36 unified Top/Bottom/Left/RightPanel into one `Panel` type;
        // `Panel::bottom` is the constructor that used to be `TopBottomPanel::bottom`.
        egui::Panel::bottom("command_bar")
            .exact_size(bar_height)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    if let Some(completions) = completions {
                        let selected = match content {
                            CommandBarContent::Editing { selected, .. } => *selected,
                            _ => None,
                        };
                        ui.horizontal(|ui| {
                            for (i, completion) in completions.iter().enumerate() {
                                let label = if completion.usage.is_empty() {
                                    completion.name.to_string()
                                } else {
                                    format!("{} {}", completion.name, completion.usage)
                                };
                                let color = if Some(i) == selected {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::GRAY
                                };
                                ui.colored_label(color, label);
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::GRAY, active_tool_name);
                        ui.separator();
                        match content {
                            CommandBarContent::Hidden => {}
                            CommandBarContent::Editing { text, .. } => {
                                ui.monospace(format!(":{text}"));
                            }
                            CommandBarContent::Status { message, is_error } => {
                                let color = if *is_error {
                                    egui::Color32::from_rgb(255, 120, 120)
                                } else {
                                    egui::Color32::WHITE
                                };
                                ui.colored_label(color, *message);
                            }
                        }
                    });
                });
            });

        // The one interactive piece of UI this module owns — see
        // `Renderer::egui_pending_events`'s doc comment: real input is forwarded here
        // only while this is open.
        if let Some(color) = color_picker.as_deref_mut() {
            let mut color32 = color_to_color32(*color);
            egui::Window::new("Color")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    egui::color_picker::color_picker_color32(
                        ui,
                        &mut color32,
                        egui::color_picker::Alpha::Opaque,
                    );
                });
            *color = color32_to_color(color32);
        }
    })
}

/// A GPU vertex buffer that's grown (reallocated, at 1.5x whatever size was actually
/// needed, to absorb small repeated growth without reallocating every time) only when new
/// data doesn't already fit, and just overwritten in place (`queue.write_buffer`)
/// otherwise — see [`upload_dynamic`]'s doc comment for why this replaced a fresh
/// `create_buffer_init` every frame (Phase 6 profiling, PLAN.md: measured to dominate
/// frame cost at any real board size — see the ledger).
struct DynamicVertexBuffer {
    buffer: wgpu::Buffer,
    capacity_bytes: u64,
}

impl DynamicVertexBuffer {
    fn new(device: &wgpu::Device, label: &str) -> Self {
        // Starts at the floor `upload_dynamic` would grow to from empty anyway — avoids a
        // zero-size buffer (wgpu requires non-zero) without over-allocating up front.
        let capacity_bytes = 16;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity_bytes,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            capacity_bytes,
        }
    }
}

/// Uploads `vertices` into `target`, growing its buffer only when the data doesn't already
/// fit (see [`DynamicVertexBuffer`]) rather than allocating a fresh GPU buffer every call
/// the way `create_buffer_init` would. A free function, not a method on
/// `DynamicVertexBuffer`, so callers can pass `&self.device`/`&self.queue` (sibling
/// `Renderer` fields) alongside `&mut self.some_buffer` without the borrow checker
/// treating `self` as borrowed as a whole the way it would through a method call on one of
/// its fields.
fn upload_dynamic<V: bytemuck::Pod + bytemuck::Zeroable>(
    target: &mut DynamicVertexBuffer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    vertices: &[V],
) -> u32 {
    let padded;
    let vertices = if vertices.is_empty() {
        padded = [V::zeroed()];
        &padded[..]
    } else {
        vertices
    };
    let bytes: &[u8] = bytemuck::cast_slice(vertices);
    let needed = bytes.len() as u64;
    if needed > target.capacity_bytes {
        target.capacity_bytes = needed.saturating_mul(3) / 2;
        target.buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: target.capacity_bytes,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
    queue.write_buffer(&target.buffer, 0, bytes);
    vertices.len() as u32
}

/// Uploads already-decoded RGBA8 pixels (`4 * width * height` bytes, row-major) as a new
/// GPU texture — used for background images now; a natural fit for scene-level image
/// elements too once those exist, so this stays a free function rather than inlined into
/// `set_background_image`.
fn upload_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: &[u8],
    width: u32,
    height: u32,
    label: &str,
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    texture
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    msaa_view: wgpu::TextureView,
    /// The pass's `LoadOp::Clear` value — the actual background when `BackgroundMode` is
    /// `Solid` (recomputed from it fresh each frame, see `render`), otherwise just a
    /// backdrop underneath the gradient/image quad (which should fully cover it, but the
    /// pass still needs a valid clear color either way — e.g. before any image has been
    /// uploaded via `set_background_image`).
    fallback_clear_color: wgpu::Color,
    grid_pipeline: wgpu::RenderPipeline,
    /// Uniform theme-colored `LineList` — the UI-only overlays (selection boxes, the
    /// eraser cursor), not board content.
    preview_pipeline: wgpu::RenderPipeline,
    /// Per-vertex colored `TriangleList` — committed strokes/shapes and the shape tools'
    /// live preview.
    colored_stroke_pipeline: wgpu::RenderPipeline,
    /// Per-vertex colored `LineList` — the pen's raw, not-yet-tessellated live preview.
    colored_preview_pipeline: wgpu::RenderPipeline,
    /// `BackgroundMode::Gradient`'s pipeline: `vs_fullscreen`/`fs_background_gradient`, a
    /// full-viewport quad, no camera transform (a gradient always covers the screen).
    background_gradient_pipeline: wgpu::RenderPipeline,
    /// `BackgroundMode::Image(ImageFit::ZoomWithCamera)`'s pipeline: `vs_board_tex`
    /// (camera-transformed) / `fs_background_image`.
    background_image_zoom_pipeline: wgpu::RenderPipeline,
    /// `BackgroundMode::Image(ImageFit::Tile)`'s pipeline: `vs_fullscreen` (no camera
    /// transform) / `fs_background_image`, with a scaled-up `uv` relying on
    /// `background_sampler`'s repeat wrap to tile.
    background_image_tile_pipeline: wgpu::RenderPipeline,
    background_gradient_buffer: wgpu::Buffer,
    background_gradient_bind_group: wgpu::BindGroup,
    background_texture_bind_group_layout: wgpu::BindGroupLayout,
    /// Repeat-address, linear-filtered — used by both background-image fit modes (tile
    /// relies on the repeat wrap; zoom-with-camera never samples outside `[0, 1]` so
    /// repeat vs. clamp makes no visible difference there).
    background_sampler: wgpu::Sampler,
    /// Set by `set_background_image`, read by `render` whenever `BackgroundMode::Image`
    /// is active; `None` (nothing uploaded yet) draws nothing, falling through to
    /// `fallback_clear_color`.
    background_image: Option<BackgroundImage>,
    background_quad_buffer: DynamicVertexBuffer,
    /// Compiled LaTeX math spans, keyed by their exact source string — set by
    /// `set_math_texture` (`blueboard-app` calls it once per new/uncached source, via
    /// `blueboard_app::latex`), read by `render` when painting a
    /// `text_markup::MathToken::Math` run. A source not yet in here (not yet attempted, or
    /// `blueboard-app` gave up on it) falls back to painting its raw literal text instead
    /// of an image — see `render`'s text-run resolution.
    math_textures: std::collections::HashMap<String, MathTexture>,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    theme_bind_group: wgpu::BindGroup,
    egui_ctx: egui::Context,
    egui_renderer: egui_wgpu::Renderer,
    /// Real pointer/keyboard events forwarded from `blueboard-app`'s winit handling,
    /// queued here and drained into egui's `RawInput` on the next `render()` call — only
    /// pushed while the color picker is open, since that's the one piece of UI that
    /// actually needs to be interactive (every other use of egui here, the command bar,
    /// is pure display; `blueboard-app` owns all real input handling otherwise — see
    /// this module's doc comment).
    egui_pending_events: Vec<egui::Event>,
    grid_buffer: DynamicVertexBuffer,
    stroke_buffer: DynamicVertexBuffer,
    colored_preview_buffer: DynamicVertexBuffer,
    uniform_preview_buffer: DynamicVertexBuffer,
    /// Per-element tessellated vertices, reused across frames while an element's path,
    /// color, and the current tessellation tolerance (zoom-dependent — see
    /// `STROKE_TESSELLATION_TOLERANCE_PX`) all stay the same — which they usually do for
    /// most elements on any given frame; only an actually-edited element or an
    /// actually-changed zoom level needs retessellating. Keyed by element id (stable,
    /// never reused — see `Scene`), pruned of ids no longer in the scene once per frame so
    /// a deleted element's old vertices don't accumulate forever.
    tessellation_cache: std::collections::HashMap<u64, CachedTessellation>,
}

/// The currently-uploaded background image — see `Renderer::background_image`'s doc
/// comment and `Renderer::set_background_image`.
struct BackgroundImage {
    bind_group: wgpu::BindGroup,
    width: f32,
    height: f32,
}

/// One compiled LaTeX math span, registered with `egui`'s own texture space (not the raw
/// `wgpu::Texture` — painting it goes through the `egui`-layered pass the same way scene
/// text already does, see `text::paint_runs`) via `egui_wgpu::Renderer::
/// register_native_texture`. `width`/`height` are the image's native pixel size, used to
/// preserve aspect ratio when `text::paint_runs` scales it to the surrounding text's
/// font size.
struct MathTexture {
    texture_id: egui::TextureId,
    width: f32,
    height: f32,
}

/// One [`Renderer`]'s `tessellation_cache` entry — see that field's doc comment for the
/// invalidation rule.
struct CachedTessellation {
    path: kurbo::BezPath,
    color: [f32; 3],
    /// The exact tessellation tolerance this was built with, compared bit-for-bit (not
    /// "close enough") — the same zoom reliably reproduces the identical `f32` tolerance,
    /// so this is a correct cache-hit test, not a fuzzy approximation.
    tolerance_bits: u32,
    vertices: Vec<ColorVertex>,
}

impl Renderer {
    pub async fn new<W>(window: Arc<W>, size: (u32, u32), theme: ThemeColors) -> Self
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .expect("create wgpu surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .expect("no suitable GPU adapter found");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("failed to create wgpu device");

        // wgpu's default behavior for an uncaptured device error (a validation error from
        // any draw/buffer/pipeline call that isn't inside a push_error_scope) is to treat
        // it as fatal and panic the whole process — confirmed the hard way by the Phase 1
        // shader bug, whose log literally read "Handling wgpu errors as fatal by default"
        // right before the panic. Logging instead turns "the app vanished, no idea why"
        // into a diagnosable line in the log file.
        device.on_uncaptured_error(Arc::new(|error| {
            log::error!("wgpu uncaptured error: {error}");
        }));
        device.set_device_lost_callback(|reason, message| {
            log::error!("wgpu device lost: {reason:?}: {message}");
        });

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: std::mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        // Theme colors are fixed for the process's lifetime (no live reload yet), so the
        // uniform buffer is written once here and never again.
        let theme_uniform: ThemeUniform = theme.into();
        let theme_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("theme uniform"),
            contents: bytemuck::bytes_of(&theme_uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let theme_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("theme bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let theme_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("theme bind group"),
            layout: &theme_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: theme_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("board shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[
                Some(&camera_bind_group_layout),
                Some(&theme_bind_group_layout),
            ],
            immediate_size: 0,
        });

        // Every pipeline here shares a pipeline layout (camera + theme bind groups,
        // whether or not a given fragment shader actually reads the theme uniform) — only
        // the vertex layout/shader entry point, primitive topology, and fragment entry
        // point differ, so one helper builds all four instead of repeating ~25 lines each.
        let build_pipeline = |label: &str,
                              vs_entry: &str,
                              vertex_layout: wgpu::VertexBufferLayout<'static>,
                              topology: wgpu::PrimitiveTopology,
                              fs_entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs_entry),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs_entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            })
        };

        let grid_pipeline = build_pipeline(
            "grid pipeline",
            "vs_main",
            Vertex::layout(),
            wgpu::PrimitiveTopology::LineList,
            "fs_grid",
        );
        let preview_pipeline = build_pipeline(
            "stroke preview pipeline",
            "vs_main",
            Vertex::layout(),
            wgpu::PrimitiveTopology::LineList,
            "fs_stroke",
        );
        let colored_stroke_pipeline = build_pipeline(
            "colored stroke pipeline",
            "vs_colored",
            ColorVertex::layout(),
            wgpu::PrimitiveTopology::TriangleList,
            "fs_colored",
        );
        let colored_preview_pipeline = build_pipeline(
            "colored preview pipeline",
            "vs_colored",
            ColorVertex::layout(),
            wgpu::PrimitiveTopology::LineList,
            "fs_colored",
        );

        // Background pipelines: a separate pipeline layout each, since they bind group 2
        // or 3 instead of (or in addition to) the camera/theme groups `build_pipeline`'s
        // shared layout assumes — see `shader.wgsl`'s background section doc comment for
        // why the gradient uniform and the image texture+sampler can't share a binding
        // group despite both being "group 2 or 3 of a background pipeline".
        let background_gradient_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("background gradient bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let background_gradient_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("background gradient uniform"),
            size: std::mem::size_of::<BgGradientUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let background_gradient_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("background gradient bind group"),
            layout: &background_gradient_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: background_gradient_buffer.as_entire_binding(),
            }],
        });
        let background_gradient_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("background gradient pipeline layout"),
                bind_group_layouts: &[None, None, Some(&background_gradient_bind_group_layout)],
                immediate_size: 0,
            });
        let background_gradient_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("background gradient pipeline"),
                layout: Some(&background_gradient_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[Some(TexVertex::layout())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_background_gradient"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            });

        let background_texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("background texture bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let background_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("background sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let background_image_zoom_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("background image zoom pipeline layout"),
                bind_group_layouts: &[
                    Some(&camera_bind_group_layout),
                    None,
                    None,
                    Some(&background_texture_bind_group_layout),
                ],
                immediate_size: 0,
            });
        let background_image_tile_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("background image tile pipeline layout"),
                bind_group_layouts: &[
                    None,
                    None,
                    None,
                    Some(&background_texture_bind_group_layout),
                ],
                immediate_size: 0,
            });
        let build_background_image_pipeline =
            |label: &str, layout: &wgpu::PipelineLayout, vs_entry: &str| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs_entry),
                        compilation_options: Default::default(),
                        buffers: &[Some(TexVertex::layout())],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_background_image"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: config.format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState {
                        count: SAMPLE_COUNT,
                        mask: !0,
                        alpha_to_coverage_enabled: false,
                    },
                    multiview_mask: None,
                    cache: None,
                })
            };
        let background_image_zoom_pipeline = build_background_image_pipeline(
            "background image zoom pipeline",
            &background_image_zoom_pipeline_layout,
            "vs_board_tex",
        );
        let background_image_tile_pipeline = build_background_image_pipeline(
            "background image tile pipeline",
            &background_image_tile_pipeline_layout,
            "vs_fullscreen",
        );

        let msaa_view = create_msaa_view(&device, &config);

        let egui_ctx = egui::Context::default();
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        let grid_buffer = DynamicVertexBuffer::new(&device, "grid vertices");
        let stroke_buffer = DynamicVertexBuffer::new(&device, "stroke vertices");
        let colored_preview_buffer = DynamicVertexBuffer::new(&device, "colored preview vertices");
        let uniform_preview_buffer = DynamicVertexBuffer::new(&device, "uniform preview vertices");
        let background_quad_buffer = DynamicVertexBuffer::new(&device, "background quad vertices");

        Self {
            surface,
            device,
            queue,
            config,
            msaa_view,
            fallback_clear_color: wgpu::Color {
                r: theme.board[0] as f64,
                g: theme.board[1] as f64,
                b: theme.board[2] as f64,
                a: 1.0,
            },
            grid_pipeline,
            preview_pipeline,
            colored_stroke_pipeline,
            colored_preview_pipeline,
            background_gradient_pipeline,
            background_image_zoom_pipeline,
            background_image_tile_pipeline,
            background_gradient_buffer,
            background_gradient_bind_group,
            background_texture_bind_group_layout,
            background_sampler,
            background_image: None,
            math_textures: std::collections::HashMap::new(),
            background_quad_buffer,
            camera_buffer,
            camera_bind_group,
            theme_bind_group,
            egui_ctx,
            egui_renderer,
            egui_pending_events: Vec::new(),
            grid_buffer,
            stroke_buffer,
            colored_preview_buffer,
            uniform_preview_buffer,
            tessellation_cache: std::collections::HashMap::new(),
        }
    }

    /// Queues a real pointer/keyboard event for egui to see on the next `render()` call —
    /// `blueboard-app` calls this while the color picker is open, so its "traditional"
    /// drag-to-pick widget is actually interactive. See `egui_pending_events`'s doc
    /// comment on why this is the one exception to egui not owning input.
    pub fn push_egui_event(&mut self, event: egui::Event) {
        self.egui_pending_events.push(event);
    }

    pub fn resize(&mut self, new_size: (u32, u32)) {
        self.config.width = new_size.0.max(1);
        self.config.height = new_size.1.max(1);
        self.surface.configure(&self.device, &self.config);
        self.msaa_view = create_msaa_view(&self.device, &self.config);
    }

    pub fn viewport_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Decodes-and-uploads `rgba` (already-decoded pixels — `blueboard-app` owns the
    /// `image` crate decode step via `:background image <path>`, this just uploads) as
    /// the new background image, replacing whatever was uploaded before. Doesn't itself
    /// switch `BackgroundMode` to `Image` — the caller still needs to set `app.background`
    /// so the next `render` call actually draws it (same split `set_background_image`'s
    /// doc comment on `Renderer::background_image` describes).
    pub fn set_background_image(&mut self, rgba: &[u8], width: u32, height: u32) {
        let texture = upload_rgba_texture(
            &self.device,
            &self.queue,
            rgba,
            width,
            height,
            "background image",
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("background image bind group"),
            layout: &self.background_texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.background_sampler),
                },
            ],
        });
        self.background_image = Some(BackgroundImage {
            bind_group,
            width: width as f32,
            height: height as f32,
        });
    }

    /// Parses `content` and resolves each `MathToken::Math` against `self.math_textures`
    /// — a compiled span becomes a `TextRun::Image`, an uncompiled one (not attempted yet,
    /// or `blueboard-app` gave up on it) falls back to its raw literal source as plain
    /// text, so a math span is never just silently missing.
    fn resolve_text_runs(&self, content: &str) -> Vec<text::TextRun> {
        blueboard_core::text_markup::parse(content)
            .into_iter()
            .map(|token| match token {
                blueboard_core::text_markup::MathToken::Text { text, bold } => {
                    text::TextRun::Text { text, bold }
                }
                blueboard_core::text_markup::MathToken::Math(source) => {
                    match self.math_textures.get(&source) {
                        Some(math) => text::TextRun::Image {
                            texture_id: math.texture_id,
                            width: math.width,
                            height: math.height,
                        },
                        None => text::TextRun::Text {
                            text: source,
                            bold: false,
                        },
                    }
                }
            })
            .collect()
    }

    /// Uploads a compiled LaTeX math span's pixels and registers them with `egui`'s
    /// texture space — see `MathTexture`'s doc comment. Called once per new source string;
    /// re-registering an already-present `key` would leak the old `egui::TextureId`'s GPU
    /// resources (egui has no "replace" call, only register/free), so this is a plain
    /// `insert` on the assumption `blueboard-app` only calls it for genuinely new keys
    /// (true given `has_math_texture`'s gating, but worth noting since nothing here
    /// enforces it).
    pub fn set_math_texture(&mut self, key: String, rgba: &[u8], width: u32, height: u32) {
        let texture = upload_rgba_texture(
            &self.device,
            &self.queue,
            rgba,
            width,
            height,
            "math texture",
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texture_id = self.egui_renderer.register_native_texture(
            &self.device,
            &view,
            wgpu::FilterMode::Linear,
        );
        self.math_textures.insert(
            key,
            MathTexture {
                texture_id,
                width: width as f32,
                height: height as f32,
            },
        );
    }

    /// Render one frame: the grid, every element in `scene` intersecting the camera's
    /// current viewport, the active tool's live `preview` if any, and the `:`-command bar
    /// on top. Takes a [`Frame`] rather than its own ever-growing argument list — plain
    /// positional params worked while this was camera/scene/preview, but kept growing one
    /// at a time (the command bar, the active tool's name, grid settings, the color
    /// picker...) until clippy's `too_many_arguments` finally said so.
    pub fn render(&mut self, frame: Frame<'_>) -> RenderOutcome {
        let Frame {
            camera,
            scene,
            preview,
            command_bar,
            active_tool_name,
            grid,
            background,
            text_input,
            color_picker,
            scale_factor,
        } = frame;
        // `get_current_texture` returns an enum rather than a `Result` as of wgpu 30 — the
        // "try again next frame" cases (Timeout/Occluded) and the "reconfigure first" cases
        // (Outdated/Lost) are both expected conditions, not necessarily program errors.
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return RenderOutcome::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("surface outdated/lost, reconfiguring");
                return RenderOutcome::NeedsReconfigure;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("wgpu validation error acquiring surface texture");
                return RenderOutcome::Skipped;
            }
        };

        let viewport = glam::Vec2::new(self.config.width as f32, self.config.height as f32);
        let view_matrix = camera.view_matrix(viewport);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&view_matrix.to_cols_array()),
        );

        // Viewport culling (PLAN.md Phase 2): only strokes whose bounding box intersects
        // the visible board-space rectangle get tessellated at all. The same board
        // min/max also bounds the grid (below) — regenerated fresh every frame to exactly
        // this extent, which is what makes it look infinite: it always covers whatever's
        // visible, at any zoom or pan, rather than only ever back to a fixed start size.
        let viewport_f64 = DVec2::new(self.config.width as f64, self.config.height as f64);
        let board_min = camera.screen_to_board(DVec2::ZERO, viewport_f64);
        let board_max = camera.screen_to_board(viewport_f64, viewport_f64);

        let grid_vertices = if grid.visible {
            match grid.style {
                GridStyle::Lines => generate_grid_lines(board_min, board_max, grid.spacing),
                GridStyle::HorizontalOnly => {
                    generate_grid_horizontal(board_min, board_max, grid.spacing)
                }
                GridStyle::Hex => generate_grid_hex(board_min, board_max, grid.spacing),
                GridStyle::Dots => generate_grid_dots(board_min, board_max, grid.spacing),
            }
        } else {
            Vec::new()
        };
        let grid_count = upload_dynamic(
            &mut self.grid_buffer,
            &self.device,
            &self.queue,
            "grid vertices",
            &grid_vertices,
        );

        // Screen-space tolerance -> board-space: tighter flattening the further zoomed in,
        // per planning/math.tex §3, instead of a fixed board-space tolerance that would
        // look increasingly faceted as zoom increases.
        let tessellation_tolerance = STROKE_TESSELLATION_TOLERANCE_PX / camera.zoom as f32;

        // Sorted by id (draw/creation order) before tessellating: `elements_in_viewport`
        // comes from the R-tree, whose iteration order is spatial, not insertion order —
        // with every element the same color this was invisible, but once elements can
        // differ in color, drawing them in an arbitrary order means a freshly-drawn
        // stroke can render *underneath* an older one it overlaps, instead of on top like
        // it should.
        let mut visible_elements: Vec<_> =
            scene.elements_in_viewport(board_min, board_max).collect();
        visible_elements.sort_by_key(|element| element.id);

        // Cache hit = same path, color, and tessellation tolerance as last time this id
        // was tessellated — see `Renderer::tessellation_cache`'s doc comment. Most
        // elements on most frames hit: panning/zooming-without-changing-zoom doesn't
        // touch any element's path or color, only which ones are currently visible.
        let tolerance_bits = tessellation_tolerance.to_bits();
        let mut stroke_vertices = Vec::new();
        // A `Text` element's `path` is only its bounding box, not a stroke outline (see
        // `ElementKind`'s doc comment) — it's painted separately, through `egui`, below.
        let mut text_draws: Vec<SceneTextDraw> = Vec::new();
        for element in &visible_elements {
            let ElementKind::Text { content, font_size } = &element.kind else {
                let cache_hit = self
                    .tessellation_cache
                    .get(&element.id)
                    .is_some_and(|cached| {
                        cached.tolerance_bits == tolerance_bits
                            && cached.color == element.color
                            && cached.path == element.path
                    });
                if !cache_hit {
                    let mut vertices = Vec::new();
                    tessellate_colored_stroke(
                        &element.path,
                        element.color,
                        STROKE_WIDTH,
                        tessellation_tolerance,
                        &mut vertices,
                    );
                    self.tessellation_cache.insert(
                        element.id,
                        CachedTessellation {
                            path: element.path.clone(),
                            color: element.color,
                            tolerance_bits,
                            vertices,
                        },
                    );
                }
                stroke_vertices.extend_from_slice(&self.tessellation_cache[&element.id].vertices);
                continue;
            };
            let bbox = element.path.bounding_box();
            let top_left = DVec2::new(bbox.x0, bbox.y0);
            let screen = camera.board_to_screen(top_left, viewport_f64);
            text_draws.push(SceneTextDraw {
                screen_pos: egui::pos2(
                    (screen.x / scale_factor as f64) as f32,
                    (screen.y / scale_factor as f64) as f32,
                ),
                runs: self.resolve_text_runs(content),
                font_size_px: font_size * camera.zoom as f32 / scale_factor,
                color: color_to_color32(element.color),
            });
        }
        // The in-progress text element (if composing one), painted the same way — a
        // trailing "|" so there's visible feedback even on an empty/all-math in-progress
        // string, not a real blinking caret (kept simple, see `TextInputPreview`'s doc
        // comment on who owns this state).
        if let Some(input) = &text_input {
            let screen = camera.board_to_screen(input.board_pos, viewport_f64);
            let mut content = input.content.to_string();
            content.push('|');
            text_draws.push(SceneTextDraw {
                screen_pos: egui::pos2(
                    (screen.x / scale_factor as f64) as f32,
                    (screen.y / scale_factor as f64) as f32,
                ),
                runs: self.resolve_text_runs(&content),
                font_size_px: input.font_size * camera.zoom as f32 / scale_factor,
                color: color_to_color32(input.color),
            });
        }
        // Drops any id no longer in the scene (deleted, erased, undone-away) so the cache
        // doesn't grow unbounded over a long editing session.
        self.tessellation_cache
            .retain(|id, _| scene.element(*id).is_some());

        let stroke_count = upload_dynamic(
            &mut self.stroke_buffer,
            &self.device,
            &self.queue,
            "stroke vertices",
            &stroke_vertices,
        );

        // The pen's raw preview draws as thin colored lines; a shape tool's preview draws
        // tessellated and colored, exactly like a committed element — see `ScenePreview`'s
        // doc comment. The UI-only overlays (Selection/EraseCursor/MultiSelection) stay on
        // the separate uniform theme-colored path — they aren't board content and
        // shouldn't track whatever color the user's currently drawing with.
        let mut colored_preview_vertices: Vec<ColorVertex> = Vec::new();
        let mut colored_preview_pipeline = &self.colored_preview_pipeline;
        let mut uniform_preview_vertices: Vec<Vertex> = Vec::new();
        match preview {
            ScenePreview::None => {}
            ScenePreview::RawPoints { points, color } => {
                colored_preview_vertices = colored_preview_line_vertices(points, color);
                colored_preview_pipeline = &self.colored_preview_pipeline;
            }
            ScenePreview::Path { path, color } => {
                tessellate_colored_stroke(
                    path,
                    color,
                    STROKE_WIDTH,
                    tessellation_tolerance,
                    &mut colored_preview_vertices,
                );
                colored_preview_pipeline = &self.colored_stroke_pipeline;
            }
            ScenePreview::Selection(rect) => {
                uniform_preview_vertices = selection_rect_vertices(rect);
            }
            ScenePreview::EraseCursor { center, radius } => {
                uniform_preview_vertices = erase_cursor_vertices(center, radius);
            }
            ScenePreview::MultiSelection(rects) => {
                uniform_preview_vertices = rects
                    .iter()
                    .copied()
                    .flat_map(selection_rect_vertices)
                    .collect();
            }
        }
        let colored_preview_count = upload_dynamic(
            &mut self.colored_preview_buffer,
            &self.device,
            &self.queue,
            "colored preview vertices",
            &colored_preview_vertices,
        );
        let uniform_preview_count = upload_dynamic(
            &mut self.uniform_preview_buffer,
            &self.device,
            &self.queue,
            "uniform preview vertices",
            &uniform_preview_vertices,
        );

        // `BackgroundMode::Solid`'s color *is* the pass's clear color — no extra geometry.
        // Gradient/image modes still need a valid clear color underneath them (the quad
        // should fully cover it, but the pass descriptor requires one regardless, and
        // nothing has been uploaded yet is a real case for `Image`), so they fall back to
        // `fallback_clear_color`.
        let clear_color = match background {
            BackgroundMode::Solid(color) => wgpu::Color {
                r: color[0] as f64,
                g: color[1] as f64,
                b: color[2] as f64,
                a: 1.0,
            },
            BackgroundMode::Gradient { .. } | BackgroundMode::Image(_) => self.fallback_clear_color,
        };

        // Computed before the pass opens (not inside it): uploading the gradient uniform
        // / background quad needs `&self.device`/`&self.queue`, which can't coexist with
        // the pass's borrow of other `self` fields if interleaved with pipeline/bind-group
        // calls on `pass` itself. `BackgroundDraw` just records which pipeline to use (if
        // any) — the actual bind group lookup happens again inside the pass block below,
        // since a borrowed reference into `self.background_image` can't be stashed here
        // without fighting the pass's own borrow of `self`.
        enum BackgroundDraw {
            None,
            Gradient,
            Image { zoom_with_camera: bool },
        }
        let background_draw = match background {
            BackgroundMode::Solid(_) => BackgroundDraw::None,
            BackgroundMode::Gradient {
                from,
                to,
                angle_deg,
            } => {
                let radians = (angle_deg as f64).to_radians();
                let uniform = BgGradientUniform {
                    from_color: [from[0], from[1], from[2], 1.0],
                    to_color: [to[0], to[1], to[2], 1.0],
                    dir: [radians.cos() as f32, radians.sin() as f32],
                    _pad: [0.0, 0.0],
                };
                self.queue.write_buffer(
                    &self.background_gradient_buffer,
                    0,
                    bytemuck::bytes_of(&uniform),
                );
                let quad = fullscreen_quad_vertices([1.0, 1.0]);
                upload_dynamic(
                    &mut self.background_quad_buffer,
                    &self.device,
                    &self.queue,
                    "background quad vertices",
                    &quad,
                );
                BackgroundDraw::Gradient
            }
            BackgroundMode::Image(fit) => match &self.background_image {
                None => BackgroundDraw::None,
                Some(image) => {
                    let (quad, zoom_with_camera) = match fit {
                        ImageFit::ZoomWithCamera => {
                            (board_image_quad_vertices(image.width, image.height), true)
                        }
                        ImageFit::Tile => {
                            let uv_scale = [
                                self.config.width as f32 / image.width.max(1.0),
                                self.config.height as f32 / image.height.max(1.0),
                            ];
                            (fullscreen_quad_vertices(uv_scale), false)
                        }
                    };
                    upload_dynamic(
                        &mut self.background_quad_buffer,
                        &self.device,
                        &self.queue,
                        "background quad vertices",
                        &quad,
                    );
                    BackgroundDraw::Image { zoom_with_camera }
                }
            },
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("board pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        // Only the resolved (single-sample) texture needs to survive the
                        // pass; discarding the multisampled one avoids writing it back.
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &self.theme_bind_group, &[]);

            match background_draw {
                BackgroundDraw::None => {}
                BackgroundDraw::Gradient => {
                    pass.set_pipeline(&self.background_gradient_pipeline);
                    pass.set_bind_group(2, &self.background_gradient_bind_group, &[]);
                    pass.set_vertex_buffer(0, self.background_quad_buffer.buffer.slice(..));
                    pass.draw(0..6, 0..1);
                }
                BackgroundDraw::Image { zoom_with_camera } => {
                    // `background_draw` was only ever set to `Image` when
                    // `self.background_image` was `Some` at the time (see above) — it
                    // can't have changed in between (nothing here mutates it).
                    let image = self
                        .background_image
                        .as_ref()
                        .expect("BackgroundDraw::Image implies background_image was Some");
                    pass.set_pipeline(if zoom_with_camera {
                        &self.background_image_zoom_pipeline
                    } else {
                        &self.background_image_tile_pipeline
                    });
                    pass.set_bind_group(3, &image.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.background_quad_buffer.buffer.slice(..));
                    pass.draw(0..6, 0..1);
                }
            }

            if grid_count > 0 {
                pass.set_pipeline(&self.grid_pipeline);
                pass.set_vertex_buffer(0, self.grid_buffer.buffer.slice(..));
                pass.draw(0..grid_count, 0..1);
            }

            if stroke_count > 0 {
                pass.set_pipeline(&self.colored_stroke_pipeline);
                pass.set_vertex_buffer(0, self.stroke_buffer.buffer.slice(..));
                pass.draw(0..stroke_count, 0..1);
            }

            if colored_preview_count > 0 {
                pass.set_pipeline(colored_preview_pipeline);
                pass.set_vertex_buffer(0, self.colored_preview_buffer.buffer.slice(..));
                pass.draw(0..colored_preview_count, 0..1);
            }

            if uniform_preview_count > 0 {
                pass.set_pipeline(&self.preview_pipeline);
                pass.set_vertex_buffer(0, self.uniform_preview_buffer.buffer.slice(..));
                pass.draw(0..uniform_preview_count, 0..1);
            }
        }

        // Second pass: the command bar, laid out and tessellated by egui, composited
        // directly onto the now-resolved swapchain texture (no MSAA target needed — egui
        // does its own edge anti-aliasing via feathering).
        let mut egui_output = run_command_bar_ui(
            &self.egui_ctx,
            (self.config.width, self.config.height),
            scale_factor,
            CommandBarInput {
                content: &command_bar,
                active_tool_name,
                text_draws: &text_draws,
            },
            std::mem::take(&mut self.egui_pending_events),
            color_picker,
        );
        let clipped_primitives = self
            .egui_ctx
            .tessellate(egui_output.shapes, egui_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: egui_output.pixels_per_point,
        };
        for (id, deltas) in &egui_output.textures_delta.set {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &clipped_primitives,
            &screen_descriptor,
        );
        {
            let mut egui_pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("command bar pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                // egui-wgpu's `render` wants a `'static` pass (so it can stash callback
                // resources on it); we don't use callbacks, so this is just satisfying
                // the signature, not opting into anything unsafe.
                .forget_lifetime();
            self.egui_renderer
                .render(&mut egui_pass, &clipped_primitives, &screen_descriptor);
        }
        for id in &egui_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        // `TexturesDelta` panics (debug_assert) on drop if it still holds entries —
        // iterating `.set`/`.free` above doesn't drain them, so this is required, not
        // just tidiness.
        egui_output.textures_delta.clear();

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(surface_texture);
        RenderOutcome::Rendered
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderOutcome {
    Rendered,
    /// Transient condition (e.g. the window is occluded) — just try again next frame.
    Skipped,
    /// The surface no longer matches the window; call `resize` with the current size and
    /// try again.
    NeedsReconfigure,
}
