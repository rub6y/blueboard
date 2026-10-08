//! winit event loop, egui chrome, the `:`-command line, config loading. See PLAN.md Phase 1-4.
//!
//! Phase 1 added the window, wgpu surface, and pan/zoom camera. Phase 2 added the pen.
//! Phase 3 added the file format. Phase 4 replaces the temporary `S`/`O` hotkeys with a
//! real vim-like `:`-command line (text rendered via egui in `blueboard-render`) backed by
//! a command registry, plus a config file for keybindings and theme colors.

use std::sync::Arc;

use blueboard_core::camera::Camera;
use blueboard_core::fitting::fit_stroke;
use blueboard_core::scene::Scene;
use blueboard_render::{CommandBarContent, RenderOutcome, Renderer};
use glam::DVec2;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

mod commands;
mod config;
mod logging;

/// How far (in screen pixels, at the zoom level the stroke was drawn at) the fitted curve
/// is allowed to deviate from the raw pointer samples — see `planning/math.tex`'s curve
/// fitting section. Converted to board-space units by dividing by the current zoom, so a
/// stroke looks equally precise regardless of zoom level when it was drawn.
///
/// Loose enough to smooth over a pixel or two of real mouse/trackpad jitter rather than
/// faithfully reproducing it as visible kinks — tightening this past a couple of pixels
/// trades smoothness for positional fidelity nobody can actually see.
const PEN_FIT_TOLERANCE_PX: f64 = 2.5;

/// Minimum screen-space distance between recorded pointer samples while drawing.
/// Pointer-move events arrive far more densely than meaningful hand motion, so most of
/// that density is jitter, not intent — decimating by distance (rather than keeping every
/// event) stops redundant near-duplicate points from ever reaching the curve fitter.
///
/// Decimation alone only thins point *density*, though — it doesn't touch the *position*
/// of the points that survive, so real jitter is still baked into each one. That's
/// invisible at the zoom level a stroke was drawn at (within [`PEN_FIT_TOLERANCE_PX`]) but
/// becomes visible as small regular kinks once zoomed in further. [`PEN_STREAMLINE_ALPHA`]
/// below is what actually addresses that.
const PEN_MIN_SAMPLE_DISTANCE_PX: f64 = 2.0;

/// Exponential-moving-average weight given to each new raw pointer position, applied
/// *before* decimation — same idea as the "streamline" parameter in
/// [perfect-freehand](https://github.com/steveruizok/perfect-freehand) (used by tldraw and
/// Excalidraw's pen tools): pull the recorded point toward a running average of recent
/// positions instead of recording the raw, jittery position directly. Lower = smoother but
/// laggier (recorded points trail further behind the actual cursor).
const PEN_STREAMLINE_ALPHA: f64 = 0.5;

/// Default path for `:save`/`:open` (and their keybindings) when no path argument is
/// given.
const DEFAULT_BOARD_PATH: &str = "board.bbd";

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
    /// Running exponential moving average of the cursor position while drawing — see
    /// [`PEN_STREAMLINE_ALPHA`]. What actually gets decimated/recorded, not the raw cursor.
    smoothed_cursor: DVec2,
    last_cursor: DVec2,

    config: config::Config,
    registry: commands::CommandRegistry,
    /// `true` while composing a `:`-command (between pressing `:` and Enter/Escape).
    command_line_active: bool,
    /// Everything typed after `:` so far.
    command_line_text: String,
    /// The last command's result, shown in the command bar until the next command
    /// replaces it. `bool` is "is this an error" (picks the display color).
    status: Option<(String, bool)>,
    should_quit: bool,
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
        log::info!("window created at {}x{}", size.width, size.height);
        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            (size.width, size.height),
            self.config.theme.into(),
        ));
        log::info!("renderer initialized");

        self.window = Some(window);
        self.renderer = Some(renderer);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Handled separately, before the `window`/`renderer` field borrows below: command
        // dispatch needs a whole `&mut self` (to let a `Command` mutate `self.scene`,
        // etc.), which can't coexist with those borrows of other fields of `self`.
        if let WindowEvent::KeyboardInput { event, .. } = &event {
            if event.state == ElementState::Pressed {
                self.handle_key_press(event.clone(), event_loop);
            }
            return;
        }

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
                let command_bar = if self.command_line_active {
                    CommandBarContent::Editing {
                        text: &self.command_line_text,
                    }
                } else if let Some((message, is_error)) = &self.status {
                    CommandBarContent::Status {
                        message,
                        is_error: *is_error,
                    }
                } else {
                    CommandBarContent::Hidden
                };
                let scale_factor = window.scale_factor() as f32;

                match renderer.render(
                    &self.camera,
                    &self.scene,
                    &self.current_stroke,
                    command_bar,
                    scale_factor,
                ) {
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
                    self.smoothed_cursor = self.last_cursor;
                    self.last_sample_screen = self.last_cursor;
                    let (width, height) = renderer.viewport_size();
                    let viewport = DVec2::new(width as f64, height as f64);
                    self.current_stroke
                        .push(self.camera.screen_to_board(self.last_cursor, viewport));
                } else if self.current_stroke.len() >= 2 {
                    let accuracy = PEN_FIT_TOLERANCE_PX / self.camera.zoom;
                    let raw_points = self.current_stroke.len();
                    if let Some(path) = fit_stroke(&self.current_stroke, accuracy) {
                        let id = self.scene.add_stroke(path);
                        log::info!("stroke {id} committed from {raw_points} raw samples");
                    } else {
                        log::warn!("fit_stroke returned None for {raw_points} raw samples");
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
                } else if self.drawing {
                    self.smoothed_cursor = self.smoothed_cursor.lerp(cursor, PEN_STREAMLINE_ALPHA);
                    if self.smoothed_cursor.distance(self.last_sample_screen)
                        >= PEN_MIN_SAMPLE_DISTANCE_PX
                    {
                        let (width, height) = renderer.viewport_size();
                        let viewport = DVec2::new(width as f64, height as f64);
                        self.current_stroke
                            .push(self.camera.screen_to_board(self.smoothed_cursor, viewport));
                        self.last_sample_screen = self.smoothed_cursor;
                        window.request_redraw();
                    }
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

            _ => {}
        }
    }
}

impl App {
    /// Keyboard handling lives outside `window_event`'s main match because command
    /// dispatch needs a whole `&mut self` — see the call site's comment.
    fn handle_key_press(&mut self, event: winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };

        if self.command_line_active {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    let input = std::mem::take(&mut self.command_line_text);
                    self.command_line_active = false;
                    let (name, args) = input.split_once(' ').unwrap_or((&input, ""));
                    // `self.registry` can't stay borrowed while `run` also needs `&mut
                    // self` to apply the command (e.g. replacing the scene) — swap it out
                    // for the call, then put it back.
                    let registry = std::mem::take(&mut self.registry);
                    let result = registry.run(self, name, args);
                    self.registry = registry;
                    self.status = Some(match result {
                        Ok(message) => (message, false),
                        Err(message) => (message, true),
                    });
                }
                Key::Named(NamedKey::Escape) => {
                    self.command_line_active = false;
                    self.command_line_text.clear();
                }
                Key::Named(NamedKey::Backspace) => {
                    self.command_line_text.pop();
                }
                _ => {
                    if let Some(text) = &event.text {
                        self.command_line_text
                            .extend(text.chars().filter(|c| !c.is_control()));
                    }
                }
            }
            window.request_redraw();
        } else {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Character(text) if text.as_str() == ":" => {
                    self.command_line_active = true;
                    self.command_line_text.clear();
                    window.request_redraw();
                }
                Key::Character(text) => {
                    let command_name = self.config.keybindings.get(text.as_str()).cloned();
                    if let Some(command_name) = command_name {
                        let registry = std::mem::take(&mut self.registry);
                        let result = registry.run(self, &command_name, "");
                        self.registry = registry;
                        self.status = Some(match result {
                            Ok(message) => (message, false),
                            Err(message) => (message, true),
                        });
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }

        if self.should_quit {
            // Distinguishes a deliberate `:q` exit from a crash when reading the log
            // later — a process just disappearing with no line here means something
            // else happened.
            log::info!("quit command received, exiting");
            event_loop.exit();
        }
    }
}

fn main() {
    let log_path = logging::init();
    log::info!("blueboard starting, logging to {}", log_path.display());

    let config = config::load();
    let registry = commands::CommandRegistry::with_builtin_commands();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App {
        config,
        registry,
        ..Default::default()
    };
    event_loop.run_app(&mut app).expect("event loop error");
}
