//! wgpu pipeline, tessellation + caching, viewport culling/LOD. See PLAN.md Phase 1-2.
//!
//! Phase 1 drew a static reference grid so camera pan/zoom had something to confirm
//! against. Phase 2 adds the real thing: pen strokes from `blueboard-core`'s scene graph,
//! tessellated by `lyon` into GPU triangles, queried each frame from the scene's R-tree by
//! viewport (culling is wired in now, per PLAN.md, even though boards are still small —
//! tessellation *caching* is still future-phase work, not done here).

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

/// A static reference grid in board space, just to make camera pan/zoom visible.
fn generate_grid_lines(extent: f32, spacing: f32) -> Vec<Vertex> {
    let mut verts = Vec::new();

    let mut x = -extent;
    while x <= extent {
        verts.push(Vertex {
            position: [x, -extent],
        });
        verts.push(Vertex {
            position: [x, extent],
        });
        x += spacing;
    }

    let mut y = -extent;
    while y <= extent {
        verts.push(Vertex {
            position: [-extent, y],
        });
        verts.push(Vertex {
            position: [extent, y],
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

/// Tessellate one stroke's outline into triangles, appending them (as a flat,
/// non-indexed vertex list, matching the grid pipeline's draw call) to `out`. `tolerance`
/// is in the same board-space units as `path` — callers are responsible for converting a
/// screen-space pixel tolerance by dividing by zoom (see [`STROKE_TESSELLATION_TOLERANCE_PX`]).
fn tessellate_stroke(path: &kurbo::BezPath, width: f32, tolerance: f32, out: &mut Vec<Vertex>) {
    let lyon_path = kurbo_path_to_lyon(path);
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_tolerance(tolerance)
        .with_line_join(lyon::tessellation::LineJoin::Round)
        .with_start_cap(lyon::tessellation::LineCap::Round)
        .with_end_cap(lyon::tessellation::LineCap::Round);

    let mut buffers: VertexBuffers<Vertex, u32> = VertexBuffers::new();
    let mut tessellator = StrokeTessellator::new();
    tessellator
        .tessellate_path(
            &lyon_path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, |vertex: StrokeVertex| Vertex {
                position: vertex.position().to_array(),
            }),
        )
        .expect("stroke tessellation failed");

    out.extend(
        buffers
            .indices
            .iter()
            .map(|&index| buffers.vertices[index as usize]),
    );
}

/// Build a single live-preview line strip (as a `LineList`, one segment per consecutive
/// pair) from the raw, not-yet-fitted points of a stroke still being drawn.
fn preview_line_vertices(points: &[DVec2]) -> Vec<Vertex> {
    points
        .windows(2)
        .flat_map(|pair| {
            [
                Vertex {
                    position: [pair[0].x as f32, pair[0].y as f32],
                },
                Vertex {
                    position: [pair[1].x as f32, pair[1].y as f32],
                },
            ]
        })
        .collect()
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

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    msaa_view: wgpu::TextureView,
    grid_pipeline: wgpu::RenderPipeline,
    stroke_pipeline: wgpu::RenderPipeline,
    preview_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    grid_vertex_buffer: wgpu::Buffer,
    grid_vertex_count: u32,
}

impl Renderer {
    pub async fn new<W>(window: Arc<W>, size: (u32, u32)) -> Self
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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("board shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[Some(&camera_bind_group_layout)],
            immediate_size: 0,
        });

        // The grid, committed strokes, and the in-progress stroke preview all share the
        // same vertex layout, pipeline layout, and vertex shader — only the fragment
        // entry point (color) and primitive topology (lines vs. triangles) differ, so one
        // helper builds all three pipelines instead of repeating ~25 lines three times.
        let build_pipeline = |label: &str, topology: wgpu::PrimitiveTopology, fs_entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(Vertex::layout())],
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
            wgpu::PrimitiveTopology::LineList,
            "fs_grid",
        );
        let stroke_pipeline = build_pipeline(
            "stroke pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            "fs_stroke",
        );
        let preview_pipeline = build_pipeline(
            "stroke preview pipeline",
            wgpu::PrimitiveTopology::LineList,
            "fs_stroke",
        );

        let grid_vertices = generate_grid_lines(2000.0, 100.0);
        let grid_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grid vertices"),
            contents: bytemuck::cast_slice(&grid_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let msaa_view = create_msaa_view(&device, &config);

        Self {
            surface,
            device,
            queue,
            config,
            msaa_view,
            grid_pipeline,
            stroke_pipeline,
            preview_pipeline,
            camera_buffer,
            camera_bind_group,
            grid_vertex_buffer,
            grid_vertex_count: grid_vertices.len() as u32,
        }
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
    /// non-empty, so an empty input gets a single padding vertex that's never drawn).
    fn upload_vertices(&self, label: &str, vertices: &[Vertex]) -> (wgpu::Buffer, u32) {
        let padded;
        let vertices = if vertices.is_empty() {
            padded = [Vertex {
                position: [0.0, 0.0],
            }];
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

    /// Render one frame: the grid, every stroke in `scene` intersecting the camera's
    /// current viewport, and — if a stroke is being drawn right now — a live raw-polyline
    /// preview of `live_stroke_points` (board-space, not yet curve-fitted).
    pub fn render(
        &mut self,
        camera: &Camera,
        scene: &Scene,
        live_stroke_points: &[DVec2],
    ) -> RenderOutcome {
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
                return RenderOutcome::NeedsReconfigure;
            }
            wgpu::CurrentSurfaceTexture::Validation => return RenderOutcome::Skipped,
        };

        let viewport = glam::Vec2::new(self.config.width as f32, self.config.height as f32);
        let view_matrix = camera.view_matrix(viewport);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&view_matrix.to_cols_array()),
        );

        // Viewport culling (PLAN.md Phase 2): only strokes whose bounding box intersects
        // the visible board-space rectangle get tessellated at all.
        let viewport_f64 = DVec2::new(self.config.width as f64, self.config.height as f64);
        let board_min = camera.screen_to_board(DVec2::ZERO, viewport_f64);
        let board_max = camera.screen_to_board(viewport_f64, viewport_f64);

        // Screen-space tolerance -> board-space: tighter flattening the further zoomed in,
        // per planning/math.tex §3, instead of a fixed board-space tolerance that would
        // look increasingly faceted as zoom increases.
        let tessellation_tolerance = STROKE_TESSELLATION_TOLERANCE_PX / camera.zoom as f32;

        let mut stroke_vertices = Vec::new();
        for stroke in scene.strokes_in_viewport(board_min, board_max) {
            tessellate_stroke(
                &stroke.path,
                STROKE_WIDTH,
                tessellation_tolerance,
                &mut stroke_vertices,
            );
        }
        let (stroke_buffer, stroke_count) =
            self.upload_vertices("stroke vertices", &stroke_vertices);

        let preview_vertices = preview_line_vertices(live_stroke_points);
        let (preview_buffer, preview_count) =
            self.upload_vertices("stroke preview vertices", &preview_vertices);

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
                        // The board itself: deep blue, per the project's name.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.08,
                            b: 0.20,
                            a: 1.0,
                        }),
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

            pass.set_pipeline(&self.grid_pipeline);
            pass.set_vertex_buffer(0, self.grid_vertex_buffer.slice(..));
            pass.draw(0..self.grid_vertex_count, 0..1);

            if stroke_count > 0 {
                pass.set_pipeline(&self.stroke_pipeline);
                pass.set_vertex_buffer(0, stroke_buffer.slice(..));
                pass.draw(0..stroke_count, 0..1);
            }

            if preview_count > 0 {
                pass.set_pipeline(&self.preview_pipeline);
                pass.set_vertex_buffer(0, preview_buffer.slice(..));
                pass.draw(0..preview_count, 0..1);
            }
        }

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
