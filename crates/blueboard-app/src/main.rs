//! winit event loop, egui chrome, the `:`-command line, config loading. See PLAN.md Phase 1-5.
//!
//! Phase 1 added the window, wgpu surface, and pan/zoom camera. Phase 2 added the pen.
//! Phase 3 added the file format. Phase 4 added the `:`-command line, a command registry,
//! and the config file. Phase 5 generalizes drawing into a `Tool` registry (`tools.rs`) —
//! the pen is now one tool among several (rect/ellipse/line), switchable via `:tool
//! <name>` or a keybinding, the same pattern Phase 4 established for commands.

use std::sync::Arc;

use blueboard_core::camera::Camera;
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
mod tools;

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
    last_cursor: DVec2,

    config: config::Config,
    registry: commands::CommandRegistry,
    tools: tools::ToolRegistry,
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
                let preview = self.tools.active_tool().preview();

                match renderer.render(
                    &self.camera,
                    &self.scene,
                    preview,
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
                let (width, height) = renderer.viewport_size();
                let viewport = DVec2::new(width as f64, height as f64);
                let mut ctx = tools::ToolContext {
                    camera: &self.camera,
                    scene: &mut self.scene,
                    viewport,
                };
                match state {
                    ElementState::Pressed => self
                        .tools
                        .active_tool_mut()
                        .on_pointer_down(&mut ctx, self.last_cursor),
                    ElementState::Released => self
                        .tools
                        .active_tool_mut()
                        .on_pointer_up(&mut ctx, self.last_cursor),
                }
                window.request_redraw();
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
                } else {
                    let (width, height) = renderer.viewport_size();
                    let viewport = DVec2::new(width as f64, height as f64);
                    let mut ctx = tools::ToolContext {
                        camera: &self.camera,
                        scene: &mut self.scene,
                        viewport,
                    };
                    self.tools
                        .active_tool_mut()
                        .on_pointer_move(&mut ctx, cursor);
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
                    self.run_command(name, args);
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
                    let binding = self.config.keybindings.get(text.as_str()).cloned();
                    if let Some(binding) = binding {
                        let (name, args) = binding.split_once(' ').unwrap_or((&binding, ""));
                        self.run_command(name, args);
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

    /// Runs a command by name (from the command line or a keybinding) and records the
    /// result as the next status message.
    fn run_command(&mut self, name: &str, args: &str) {
        // `self.registry` can't stay borrowed while `run` also needs `&mut self` to apply
        // the command (e.g. replacing the scene) — swap it out for the call, then put it
        // back. See `tools.rs`'s doc comment for why `Tool` dispatch doesn't need this.
        let registry = std::mem::take(&mut self.registry);
        let result = registry.run(self, name, args);
        self.registry = registry;
        self.status = Some(match result {
            Ok(message) => (message, false),
            Err(message) => (message, true),
        });
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
