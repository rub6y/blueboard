//! winit event loop, egui chrome, the `:`-command line, config loading. See PLAN.md Phase 1-4.
//!
//! Phase 1 added the window, wgpu surface, and pan/zoom camera. Phase 2 adds the pen:
//! left-drag draws a stroke (fit to a vector curve on release), middle-drag pans, scroll
//! zooms. No tool/command registries yet — this is the one hardcoded tool those later
//! phases will generalize.

use std::sync::Arc;

use blueboard_core::camera::Camera;
use blueboard_core::fitting::fit_stroke;
use blueboard_core::scene::Scene;
use blueboard_render::{RenderOutcome, Renderer};
use glam::DVec2;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// How far (in screen pixels, at the zoom level the stroke was drawn at) the fitted curve
/// is allowed to deviate from the raw pointer samples — see `planning/math.tex`'s curve
/// fitting section. Converted to board-space units by dividing by the current zoom, so a
/// stroke looks equally precise regardless of zoom level when it was drawn.
///
/// Loose enough to smooth over a pixel or two of real mouse/trackpad jitter rather than
/// faithfully reproducing it as visible kinks — tightening this past a couple of pixels
/// trades smoothness for positional fidelity nobody can actually see.
const PEN_FIT_TOLERANCE_PX: f64 = 2.5;

/// Minimum screen-space distance between recorded raw pointer samples while drawing.
/// Pointer-move events arrive far more densely than meaningful hand motion, so most of
/// that density is jitter, not intent — decimating by distance (rather than keeping every
/// event) stops that jitter from ever reaching the curve fitter as fake curvature.
const PEN_MIN_SAMPLE_DISTANCE_PX: f64 = 2.0;

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    camera: Camera,
    scene: Scene,
    panning: bool,
    drawing: bool,
    /// Board-space points of the stroke currently being drawn, in order. Cleared (and
    /// fit to a curve, added to `scene`) when the pen is lifted.
    current_stroke: Vec<DVec2>,
    /// Screen-space position of the last *recorded* sample in `current_stroke` (not
    /// necessarily the last `CursorMoved` event — see [`PEN_MIN_SAMPLE_DISTANCE_PX`]).
    last_sample_screen: DVec2,
    last_cursor: DVec2,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // winit only allows creating windows/GPU surfaces once the event loop has an
        // active platform backend, which is what `resumed` signals — doing this in `main`
        // before `run_app` is not guaranteed to work on every platform.
        if self.window.is_some() {
            return;
        }

        let window_attributes = Window::default_attributes().with_title("Blueboard");
        let window = Arc::new(
            event_loop
                .create_window(window_attributes)
                .expect("failed to create window"),
        );
        let size = window.inner_size();
        let renderer = pollster::block_on(Renderer::new(window.clone(), (size.width, size.height)));

        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                renderer.resize((size.width, size.height));
                window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                match renderer.render(&self.camera, &self.scene, &self.current_stroke) {
                    RenderOutcome::Rendered | RenderOutcome::Skipped => {}
                    RenderOutcome::NeedsReconfigure => {
                        let size = window.inner_size();
                        renderer.resize((size.width, size.height));
                        window.request_redraw();
                    }
                }
            }

            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.drawing = state == ElementState::Pressed;
                if self.drawing {
                    self.current_stroke.clear();
                    self.last_sample_screen = self.last_cursor;
                    let (width, height) = renderer.viewport_size();
                    let viewport = DVec2::new(width as f64, height as f64);
                    self.current_stroke
                        .push(self.camera.screen_to_board(self.last_cursor, viewport));
                } else if self.current_stroke.len() >= 2 {
                    let accuracy = PEN_FIT_TOLERANCE_PX / self.camera.zoom;
                    if let Some(path) = fit_stroke(&self.current_stroke, accuracy) {
                        self.scene.add_stroke(path);
                    }
                    self.current_stroke.clear();
                    window.request_redraw();
                } else {
                    self.current_stroke.clear();
                }
            }

            WindowEvent::MouseInput {
                state,
                button: MouseButton::Middle,
                ..
            } => {
                self.panning = state == ElementState::Pressed;
            }

            WindowEvent::CursorMoved { position, .. } => {
                let cursor = DVec2::new(position.x, position.y);
                if self.panning {
                    self.camera.pan_by_screen_delta(cursor - self.last_cursor);
                    window.request_redraw();
                } else if self.drawing
                    && cursor.distance(self.last_sample_screen) >= PEN_MIN_SAMPLE_DISTANCE_PX
                {
                    let (width, height) = renderer.viewport_size();
                    let viewport = DVec2::new(width as f64, height as f64);
                    self.current_stroke
                        .push(self.camera.screen_to_board(cursor, viewport));
                    self.last_sample_screen = cursor;
                    window.request_redraw();
                }
                self.last_cursor = cursor;
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // Scroll-to-zoom, anchored on the cursor so the board point under the
                // mouse stays put — matches the zoom-to-cursor feel of Miro/most editors.
                let scroll_amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(pos) => pos.y / 100.0,
                };
                let factor = 1.1_f64.powf(scroll_amount);
                let (width, height) = renderer.viewport_size();
                let viewport = DVec2::new(width as f64, height as f64);
                self.camera.zoom_by(factor, self.last_cursor, viewport);
                window.request_redraw();
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),

            _ => {}
        }
    }
}

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}
