//! winit event loop, egui chrome, the `:`-command line, config loading. See PLAN.md Phase 1-4.
//!
//! Phase 1: a window, a wgpu surface, and a pan/zoom camera over a reference grid. No UI
//! chrome, command line, or scene elements yet — those are later phases.

use std::sync::Arc;

use blueboard_core::camera::Camera;
use blueboard_render::{RenderOutcome, Renderer};
use glam::DVec2;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    camera: Camera,
    dragging: bool,
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

            WindowEvent::RedrawRequested => match renderer.render(&self.camera) {
                RenderOutcome::Rendered | RenderOutcome::Skipped => {}
                RenderOutcome::NeedsReconfigure => {
                    let size = window.inner_size();
                    renderer.resize((size.width, size.height));
                    window.request_redraw();
                }
            },

            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.dragging = state == ElementState::Pressed;
            }

            WindowEvent::CursorMoved { position, .. } => {
                let cursor = DVec2::new(position.x, position.y);
                if self.dragging {
                    self.camera.pan_by_screen_delta(cursor - self.last_cursor);
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
