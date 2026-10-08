//! wgpu pipeline, tessellation + caching, viewport culling/LOD. See PLAN.md Phase 1-2.
//!
//! Phase 1 drew a static reference grid so camera pan/zoom had something to confirm
//! against. Phase 2 adds the real thing: pen strokes from `blueboard-core`'s scene graph,
//! tessellated by `lyon` into GPU triangles, queried each frame from the scene's R-tree by
//! viewport (culling is wired in now, per PLAN.md, even though boards are still small —
//! tessellation *caching* is still future-phase work, not done here). Phase 4 adds a
//! configurable color theme and the `:`-command bar, rendered with `egui`/`egui-wgpu`
//! layered on top of the board in a second pass.

use std::sync::Arc;

use blueboard_core::camera::Camera;
use blueboard_core::scene::Scene;
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
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            visible: true,
            spacing: 100.0,
        }
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

/// Everything [`Renderer::render`] needs to draw one frame, bundled so the method takes
/// one argument instead of an ever-growing positional list.
pub struct Frame<'a> {
    pub camera: &'a Camera,
    pub scene: &'a Scene,
    pub preview: ScenePreview<'a>,
    pub command_bar: CommandBarContent<'a>,
    pub active_tool_name: &'a str,
    pub grid: GridSettings,
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
fn run_command_bar_ui(
    ctx: &egui::Context,
    viewport_size: (u32, u32),
    scale_factor: f32,
    content: &CommandBarContent<'_>,
    active_tool_name: &str,
    events: Vec<egui::Event>,
    mut color_picker: Option<&mut [f32; 3]>,
) -> egui::FullOutput {
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

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    msaa_view: wgpu::TextureView,
    board_clear_color: wgpu::Color,
    grid_pipeline: wgpu::RenderPipeline,
    /// Uniform theme-colored `LineList` — the UI-only overlays (selection boxes, the
    /// eraser cursor), not board content.
    preview_pipeline: wgpu::RenderPipeline,
    /// Per-vertex colored `TriangleList` — committed strokes/shapes and the shape tools'
    /// live preview.
    colored_stroke_pipeline: wgpu::RenderPipeline,
    /// Per-vertex colored `LineList` — the pen's raw, not-yet-tessellated live preview.
    colored_preview_pipeline: wgpu::RenderPipeline,
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

        let msaa_view = create_msaa_view(&device, &config);

        let egui_ctx = egui::Context::default();
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        Self {
            surface,
            device,
            queue,
            config,
            msaa_view,
            board_clear_color: wgpu::Color {
                r: theme.board[0] as f64,
                g: theme.board[1] as f64,
                b: theme.board[2] as f64,
                a: 1.0,
            },
            grid_pipeline,
            preview_pipeline,
            colored_stroke_pipeline,
            colored_preview_pipeline,
            camera_buffer,
            camera_bind_group,
            theme_bind_group,
            egui_ctx,
            egui_renderer,
            egui_pending_events: Vec::new(),
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

    /// Upload a vertex list as a fresh GPU buffer and return it with its vertex count (0
    /// vertices still returns a valid, zero-size-safe buffer — `wgpu` requires buffers be
    /// non-empty, so an empty input gets a single zeroed padding vertex that's never
    /// drawn). Generic over vertex type since `Vertex` and `ColorVertex` both get this
    /// same per-frame dynamic-buffer treatment.
    fn upload_vertices<V: bytemuck::Pod + bytemuck::Zeroable>(
        &self,
        label: &str,
        vertices: &[V],
    ) -> (wgpu::Buffer, u32) {
        let padded;
        let vertices = if vertices.is_empty() {
            padded = [V::zeroed()];
            &padded[..]
        } else {
            vertices
        };
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        (buffer, vertices.len() as u32)
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
            generate_grid_lines(board_min, board_max, grid.spacing)
        } else {
            Vec::new()
        };
        let (grid_buffer, grid_count) = self.upload_vertices("grid vertices", &grid_vertices);

        // Screen-space tolerance -> board-space: tighter flattening the further zoomed in,
        // per planning/math.tex §3, instead of a fixed board-space tolerance that would
        // look increasingly faceted as zoom increases.
        let tessellation_tolerance = STROKE_TESSELLATION_TOLERANCE_PX / camera.zoom as f32;

        let mut stroke_vertices = Vec::new();
        // Sorted by id (draw/creation order) before tessellating: `elements_in_viewport`
        // comes from the R-tree, whose iteration order is spatial, not insertion order —
        // with every element the same color this was invisible, but once elements can
        // differ in color, drawing them in an arbitrary order means a freshly-drawn
        // stroke can render *underneath* an older one it overlaps, instead of on top like
        // it should.
        let mut visible_elements: Vec<_> =
            scene.elements_in_viewport(board_min, board_max).collect();
        visible_elements.sort_by_key(|element| element.id);
        for element in visible_elements {
            tessellate_colored_stroke(
                &element.path,
                element.color,
                STROKE_WIDTH,
                tessellation_tolerance,
                &mut stroke_vertices,
            );
        }
        let (stroke_buffer, stroke_count) =
            self.upload_vertices("stroke vertices", &stroke_vertices);

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
        let (colored_preview_buffer, colored_preview_count) =
            self.upload_vertices("colored preview vertices", &colored_preview_vertices);
        let (uniform_preview_buffer, uniform_preview_count) =
            self.upload_vertices("uniform preview vertices", &uniform_preview_vertices);

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
                        load: wgpu::LoadOp::Clear(self.board_clear_color),
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

            if grid_count > 0 {
                pass.set_pipeline(&self.grid_pipeline);
                pass.set_vertex_buffer(0, grid_buffer.slice(..));
                pass.draw(0..grid_count, 0..1);
            }

            if stroke_count > 0 {
                pass.set_pipeline(&self.colored_stroke_pipeline);
                pass.set_vertex_buffer(0, stroke_buffer.slice(..));
                pass.draw(0..stroke_count, 0..1);
            }

            if colored_preview_count > 0 {
                pass.set_pipeline(colored_preview_pipeline);
                pass.set_vertex_buffer(0, colored_preview_buffer.slice(..));
                pass.draw(0..colored_preview_count, 0..1);
            }

            if uniform_preview_count > 0 {
                pass.set_pipeline(&self.preview_pipeline);
                pass.set_vertex_buffer(0, uniform_preview_buffer.slice(..));
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
            &command_bar,
            active_tool_name,
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
