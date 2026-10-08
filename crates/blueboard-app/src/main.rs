//! winit event loop, egui chrome, the `:`-command line, config loading. See PLAN.md Phase 1-5.
//!
//! Phase 1 added the window, wgpu surface, and pan/zoom camera. Phase 2 added the pen.
//! Phase 3 added the file format. Phase 4 added the `:`-command line, a command registry,
//! and the config file. Phase 5 generalizes drawing into a `Tool` registry (`tools.rs`) —
//! the pen is now one tool among several (rect/ellipse/line), switchable via `:tool
//! <name>` or a keybinding, the same pattern Phase 4 established for commands.

use std::sync::Arc;

use blueboard_core::camera::Camera;
use blueboard_core::history::History;
use blueboard_core::scene::Scene;
use blueboard_render::{CommandBarContent, Frame, GridSettings, RenderOutcome, Renderer};
use glam::DVec2;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

mod color;
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
    /// Undo/redo stacks for `scene` — every scene edit not made *by* the history itself
    /// is recorded (see `run_command` and the tool-dispatch arm below).
    history: History,
    panning: bool,
    last_cursor: DVec2,

    config: config::Config,
    registry: commands::CommandRegistry,
    tools: tools::ToolRegistry,
    /// `true` while composing a `:`-command (between pressing `:` and Enter/Escape).
    command_line_active: bool,
    /// Everything typed after `:` so far.
    command_line_text: String,
    /// Which Tab-completion candidate (from `command_line_completions`, below) is
    /// currently highlighted, if any. Reset to `None` whenever the user types or
    /// backspaces directly, so a fresh prefix always starts unselected — a stale index
    /// would otherwise point at an unrelated candidate once the match set changes
    /// underneath it.
    command_completion_index: Option<usize>,
    /// The last command's result, shown in the command bar until the next command
    /// replaces it. `bool` is "is this an error" (picks the display color).
    status: Option<(String, bool)>,
    should_quit: bool,

    /// What the pen and shape tools draw their *next* element in — set by `:color`,
    /// remembered per-element once drawn (an element's own color never changes just
    /// because this does). Overridden to white in `main()`'s `App` literal, since
    /// `#[derive(Default)]` would otherwise zero it to black.
    current_color: [f32; 3],
    /// Whether the `:color picker` toggle is currently showing the interactive color
    /// picker — see `handle_key_press`'s `Escape` arm (closes it in preference to
    /// quitting) and `window_event`'s pointer handlers (suppress tool dispatch and
    /// forward real input to egui while this is `true`).
    color_picker_open: bool,
    grid: GridSettings,
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
                let completions: Vec<blueboard_render::Completion> = if self.command_line_active {
                    let (_, candidates) = command_line_completions(
                        &self.command_line_text,
                        &self.registry,
                        &self.tools,
                    );
                    candidates
                        .into_iter()
                        .map(|(name, usage)| blueboard_render::Completion { name, usage })
                        .collect()
                } else {
                    Vec::new()
                };
                let command_bar = if self.command_line_active {
                    CommandBarContent::Editing {
                        text: &self.command_line_text,
                        completions: &completions,
                        selected: self.command_completion_index,
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
                let active_tool_name = self.tools.active_tool().name();
                let color_picker = self.color_picker_open.then_some(&mut self.current_color);

                match renderer.render(Frame {
                    camera: &self.camera,
                    scene: &self.scene,
                    preview,
                    command_bar,
                    active_tool_name,
                    grid: self.grid,
                    color_picker,
                    scale_factor,
                }) {
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
                // While the color picker is open, a left click is for it, not the canvas
                // — forwarded to egui below instead of dispatched to the active tool, so
                // clicking the picker can't also draw underneath it.
                if self.color_picker_open {
                    let pos = screen_to_egui_points(self.last_cursor, window.scale_factor());
                    renderer.push_egui_event(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: state == ElementState::Pressed,
                        modifiers: egui::Modifiers::default(),
                    });
                    window.request_redraw();
                    return;
                }

                let (width, height) = renderer.viewport_size();
                let viewport = DVec2::new(width as f64, height as f64);
                // Snapshot around the dispatch so a tool that commits something (a
                // stroke, a shape) becomes one undo point automatically — a tool that
                // commits nothing records nothing (`History::record` checks for a
                // change). `on_pointer_move` is deliberately *not* wrapped: it fires at
                // screen refresh rate, and a snapshot per event would cost far more than
                // the edits are worth — a future tool that mutates the scene mid-drag
                // (e.g. selection move) has to record its own edit.
                let before = self.scene.snapshot();
                let mut ctx = tools::ToolContext {
                    camera: &self.camera,
                    scene: &mut self.scene,
                    viewport,
                    color: self.current_color,
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
                self.history.record(before, &self.scene);
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
                if self.color_picker_open {
                    let pos = screen_to_egui_points(cursor, window.scale_factor());
                    renderer.push_egui_event(egui::Event::PointerMoved(pos));
                    window.request_redraw();
                } else if self.panning {
                    self.camera.pan_by_screen_delta(cursor - self.last_cursor);
                    window.request_redraw();
                } else {
                    let (width, height) = renderer.viewport_size();
                    let viewport = DVec2::new(width as f64, height as f64);
                    let mut ctx = tools::ToolContext {
                        camera: &self.camera,
                        scene: &mut self.scene,
                        viewport,
                        color: self.current_color,
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

/// Converts a screen-space cursor position (physical pixels, winit's unit) to the
/// logical-pixel "points" egui's `RawInput` events expect — same division by scale factor
/// `RedrawRequested` already does for `screen_size_points`, just for one point instead of
/// the whole viewport.
fn screen_to_egui_points(cursor: DVec2, scale_factor: f64) -> egui::Pos2 {
    egui::pos2(
        (cursor.x / scale_factor) as f32,
        (cursor.y / scale_factor) as f32,
    )
}

/// What the command line would complete right now, if anything: the fixed text to keep
/// (prepended to whatever candidate is chosen) and the matching candidates as `(name,
/// usage)` pairs. Completes the command name itself (nothing typed after it yet), or —
/// the one special case — a tool name for `:tool <prefix>`, since that's the one built-in
/// command whose argument has a fixed, known vocabulary to complete against; every other
/// command's arguments (a save/open path, say) aren't completed.
///
/// A free function, not an `App` method, deliberately: its two call sites both need it
/// alongside a `&mut self.renderer`/`&self.window` borrow already in scope (see
/// `RedrawRequested`'s comment on why command dispatch is handled the same way), and a
/// `&self` method call would conflict with that the same way a whole-`self` borrow always
/// does — passing the specific fields it needs keeps the borrows disjoint.
fn command_line_completions<'a>(
    text: &'a str,
    registry: &'a commands::CommandRegistry,
    tools: &'a tools::ToolRegistry,
) -> (&'static str, Vec<(&'static str, &'static str)>) {
    match text.split_once(' ') {
        None => ("", registry.completions(text)),
        Some(("tool", rest)) if !rest.contains(' ') => (
            "tool ",
            tools
                .completions(rest)
                .into_iter()
                .map(|name| (name, ""))
                .collect(),
        ),
        _ => ("", Vec::new()),
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
                    self.command_completion_index = None;
                }
                Key::Named(NamedKey::Tab) => {
                    let (fixed_prefix, candidates) = command_line_completions(
                        &self.command_line_text,
                        &self.registry,
                        &self.tools,
                    );
                    if !candidates.is_empty() {
                        let next = match self.command_completion_index {
                            Some(i) => (i + 1) % candidates.len(),
                            None => 0,
                        };
                        self.command_completion_index = Some(next);
                        self.command_line_text = format!("{fixed_prefix}{}", candidates[next].0);
                    }
                }
                _ => {
                    if let Some(text) = &event.text {
                        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
                        if !typed.is_empty() {
                            self.command_line_text.push_str(&typed);
                            self.command_completion_index = None;
                        }
                    }
                }
            }
            window.request_redraw();
        } else {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) if self.color_picker_open => {
                    self.color_picker_open = false;
                    window.request_redraw();
                }
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Character(text) if text.as_str() == ":" => {
                    self.command_line_active = true;
                    self.command_line_text.clear();
                    self.command_completion_index = None;
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
        // Asked before the run, while the registry is still available: this is what keeps
        // `undo`/`redo` from recording themselves as edits to undo.
        let records_undo = registry.records_undo(name);
        let before = self.scene.snapshot();
        let result = registry.run(self, name, args);
        self.registry = registry;
        if records_undo {
            self.history.record(before, &self.scene);
        }
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
        // `#[derive(Default)]` would otherwise zero this to black — see the field's doc
        // comment.
        current_color: blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
        ..Default::default()
    };
    event_loop.run_app(&mut app).expect("event loop error");
}
