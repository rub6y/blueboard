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
use blueboard_core::scene::{ElementKind, Scene};
use blueboard_render::{CommandBarContent, Frame, GridSettings, RenderOutcome, Renderer};
use glam::DVec2;
use kurbo::Shape as _;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

mod camera_macros;
mod color;
mod commands;
mod config;
mod latex;
mod logging;
mod macros;
mod text_edit;
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
    /// What to fill the board's background with — mutated by `:background`. See
    /// `BackgroundMode`'s doc comment for why `Image`'s pixel data lives in the renderer
    /// instead of here.
    background: blueboard_render::BackgroundMode,
    /// `true` while composing a text element (between a text-tool click and Enter/Escape)
    /// — same shape as `command_line_active`, see `handle_key_press`'s text-input branch.
    /// Real pointer/keyboard input bypasses normal tool dispatch while this (or the text
    /// tool itself, before the first click) is active, same precedent as
    /// `color_picker_open`.
    text_input_active: bool,
    /// Raw markdown/math source typed so far for the in-progress text element — parsed
    /// into `MathToken`s only at paint/commit time (see `tools::TextTool`'s doc comment).
    text_input_text: String,
    /// Caret position within `text_input_text`, as a **character** index (not byte offset
    /// — `text_edit::insert`/`backspace`/`delete_forward` all operate in char-index terms
    /// so multi-byte UTF-8 input never splits a character), moved by the arrow/Home/End
    /// keys and clamped to `[0, text_input_text.chars().count()]`.
    text_input_cursor: usize,
    /// Board-space point the in-progress text element's top-left corner will be placed at
    /// — captured from the click that started composing.
    text_input_board_pos: DVec2,
    /// Font size the in-progress text element will commit at — `tools::
    /// DEFAULT_TEXT_FONT_SIZE` for a fresh placement, or the existing element's own font
    /// size when re-editing one (see `text_input_editing_id`), so re-editing doesn't reset
    /// a size the user may have resized via the selection tool's handles.
    text_input_font_size: f64,
    /// `Some(id)` while the in-progress composition is *editing an existing* text element
    /// (entered via the selection tool's Enter-to-edit) rather than placing a brand new
    /// one — committing then calls `Scene::set_text_content` on this id instead of
    /// `add_text_element`, and committing to an empty string deletes it. Always `None`
    /// when `text_input_active` is `false`.
    text_input_editing_id: Option<u64>,
    /// Math spans (`text_markup::MathToken::Math`'s source string) already handed to
    /// `latex::render_math` this session, success or failure — gates `ensure_math_textures`
    /// so a broken formula isn't recompiled every single frame. Cleared for nothing: a
    /// source string is content-addressed, so there's never a reason to retry one that
    /// already failed except editing it into a different string, which is a new key.
    latex_attempted: std::collections::HashSet<String>,
    /// Tool+color macros bound to number keys, set by `:m <number>` and persisted to
    /// their own file — see `macros.rs`'s doc comment for why that's separate from
    /// `config.toml`. Loaded once at startup in `main()`.
    macros: macros::Macros,
    /// Camera (pan+zoom) macros bound to function keys, set by `:fm <number>` and
    /// persisted to their own file — see `camera_macros.rs`'s doc comment. Loaded once at
    /// startup in `main()`.
    camera_macros: camera_macros::CameraMacros,
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
                ensure_math_textures(
                    &self.scene,
                    &mut self.latex_attempted,
                    &mut self.status,
                    renderer,
                );
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
                // Captured before `color_picker`'s mutable borrow of `current_color`
                // below, since the text-input preview also needs to read it.
                let text_input_color = self.current_color;
                let color_picker = self.color_picker_open.then_some(&mut self.current_color);

                match renderer.render(Frame {
                    camera: &self.camera,
                    scene: &self.scene,
                    preview,
                    command_bar,
                    active_tool_name,
                    grid: self.grid,
                    background: self.background,
                    text_input: self.text_input_active.then(|| {
                        blueboard_render::TextInputPreview {
                            board_pos: self.text_input_board_pos,
                            content: &self.text_input_text,
                            cursor: self.text_input_cursor,
                            font_size: self.text_input_font_size as f32,
                            color: text_input_color,
                            editing_id: self.text_input_editing_id,
                        }
                    }),
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

                // The text tool's only interaction is "click to place, then type" — see
                // `tools::TextTool`'s doc comment on why this is intercepted here rather
                // than reaching normal `Tool` dispatch below (same precedent as the color
                // picker above). A click while already composing is ignored: finish
                // (Enter/Escape) before starting a new one.
                if self.tools.active_tool().name() == "text" {
                    if state == ElementState::Pressed && !self.text_input_active {
                        let (width, height) = renderer.viewport_size();
                        let viewport = DVec2::new(width as f64, height as f64);
                        self.text_input_board_pos =
                            self.camera.screen_to_board(self.last_cursor, viewport);
                        self.text_input_active = true;
                        self.text_input_text.clear();
                        self.text_input_cursor = 0;
                        self.text_input_font_size = tools::DEFAULT_TEXT_FONT_SIZE;
                        self.text_input_editing_id = None;
                    }
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

/// Maps the handful of named keys the color picker's fields actually need (Tab to move
/// between them, the rest to edit whichever's focused) — not a general winit-to-egui key
/// mapping, just enough for basic text-field use.
fn winit_key_to_egui(key: &Key) -> Option<egui::Key> {
    match key {
        Key::Named(NamedKey::Tab) => Some(egui::Key::Tab),
        Key::Named(NamedKey::Backspace) => Some(egui::Key::Backspace),
        Key::Named(NamedKey::Delete) => Some(egui::Key::Delete),
        Key::Named(NamedKey::Enter) => Some(egui::Key::Enter),
        Key::Named(NamedKey::ArrowLeft) => Some(egui::Key::ArrowLeft),
        Key::Named(NamedKey::ArrowRight) => Some(egui::Key::ArrowRight),
        Key::Named(NamedKey::ArrowUp) => Some(egui::Key::ArrowUp),
        Key::Named(NamedKey::ArrowDown) => Some(egui::Key::ArrowDown),
        Key::Named(NamedKey::Home) => Some(egui::Key::Home),
        Key::Named(NamedKey::End) => Some(egui::Key::End),
        _ => None,
    }
}

/// `"F1"`-`"F12"` for the function keys this app binds camera macros to, `None` for
/// anything else (including `F13`+, deliberately left unbound). The string form matches
/// `camera_macros::CameraMacros`'s key shape, which reads the same way in a hand-edited
/// `camera_macros.toml` (`[macros."F3"]`) a digit-macro key already does.
fn function_key_label(key: NamedKey) -> Option<&'static str> {
    Some(match key {
        NamedKey::F1 => "F1",
        NamedKey::F2 => "F2",
        NamedKey::F3 => "F3",
        NamedKey::F4 => "F4",
        NamedKey::F5 => "F5",
        NamedKey::F6 => "F6",
        NamedKey::F7 => "F7",
        NamedKey::F8 => "F8",
        NamedKey::F9 => "F9",
        NamedKey::F10 => "F10",
        NamedKey::F11 => "F11",
        NamedKey::F12 => "F12",
        _ => return None,
    })
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

/// Compiles any new (never-attempted) LaTeX math span found in `scene`'s text elements —
/// called once per `RedrawRequested`, cheap after the first pass since `attempted` makes
/// every already-seen source a no-op `HashSet` lookup. Scans every element, not just
/// what's currently visible, so panning onto previously off-screen text doesn't show raw
/// unrendered source for a frame.
///
/// A free function, not an `App` method, for the same reason `command_line_completions`
/// above is: its caller already holds `&mut self.renderer` alongside `&self.window`,
/// which a `&mut self` method call would conflict with.
fn ensure_math_textures(
    scene: &Scene,
    attempted: &mut std::collections::HashSet<String>,
    status: &mut Option<(String, bool)>,
    renderer: &mut Renderer,
) {
    let mut sources = Vec::new();
    for element in scene.all_elements() {
        if let blueboard_core::scene::ElementKind::Text { content, .. } = &element.kind {
            for token in blueboard_core::text_markup::parse(content) {
                if let blueboard_core::text_markup::MathToken::Math(source) = token {
                    sources.push(source);
                }
            }
        }
    }

    for source in sources {
        if attempted.contains(&source) {
            continue;
        }
        attempted.insert(source.clone());
        match latex::render_math(&source) {
            Ok(rendered) => {
                renderer.set_math_texture(source, &rendered.rgba, rendered.width, rendered.height);
            }
            Err(err) => {
                log::warn!("failed to render LaTeX span {source:?}: {err}");
                *status = Some((format!("latex: {err}"), true));
            }
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

        if self.text_input_active {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    let content = std::mem::take(&mut self.text_input_text);
                    self.text_input_active = false;
                    self.text_input_cursor = 0;
                    let editing_id = self.text_input_editing_id.take();
                    let before = self.scene.snapshot();
                    match (editing_id, content.trim().is_empty()) {
                        (Some(id), true) => {
                            // Edited down to nothing: delete rather than leave an
                            // unchanged stale element behind — matches "clearing a text
                            // field's content and confirming" in most editors.
                            self.scene.remove_element(id);
                        }
                        (Some(id), false) => {
                            self.scene.set_text_content(
                                id,
                                content,
                                self.text_input_font_size as f32,
                            );
                        }
                        (None, true) => {}
                        (None, false) => {
                            self.scene.add_text_element(
                                self.text_input_board_pos,
                                self.current_color,
                                content,
                                self.text_input_font_size as f32,
                            );
                        }
                    }
                    self.history.record(before, &self.scene);
                }
                Key::Named(NamedKey::Escape) => {
                    self.text_input_active = false;
                    self.text_input_text.clear();
                    self.text_input_cursor = 0;
                    self.text_input_editing_id = None;
                }
                Key::Named(NamedKey::Backspace) => {
                    text_edit::backspace(&mut self.text_input_text, &mut self.text_input_cursor);
                }
                Key::Named(NamedKey::Delete) => {
                    text_edit::delete_forward(&mut self.text_input_text, self.text_input_cursor);
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.text_input_cursor = self.text_input_cursor.saturating_sub(1);
                }
                Key::Named(NamedKey::ArrowRight) => {
                    let len = self.text_input_text.chars().count();
                    self.text_input_cursor = (self.text_input_cursor + 1).min(len);
                }
                Key::Named(NamedKey::Home) => {
                    self.text_input_cursor = 0;
                }
                Key::Named(NamedKey::End) => {
                    self.text_input_cursor = self.text_input_text.chars().count();
                }
                _ => {
                    if let Some(text) = &event.text {
                        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
                        if !typed.is_empty() {
                            text_edit::insert(
                                &mut self.text_input_text,
                                &mut self.text_input_cursor,
                                &typed,
                            );
                        }
                    }
                }
            }
            window.request_redraw();
            return;
        }

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
        } else if self.color_picker_open {
            // Keyboard focus belongs to the picker while it's open — Tab moves between
            // its fields (hex/R/G/B), text/backspace/arrows edit whichever's focused.
            // Escape closes it instead of its normal quit-the-app behavior.
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.color_picker_open = false;
                }
                _ => {
                    if let Some(renderer) = self.renderer.as_mut() {
                        if let Some(egui_key) = winit_key_to_egui(&event.logical_key) {
                            renderer.push_egui_event(egui::Event::Key {
                                key: egui_key,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers: egui::Modifiers::default(),
                            });
                        }
                        if let Some(text) = &event.text {
                            let typed: String = text.chars().filter(|c| !c.is_control()).collect();
                            if !typed.is_empty() {
                                renderer.push_egui_event(egui::Event::Text(typed));
                            }
                        }
                    }
                }
            }
            window.request_redraw();
        } else {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                // Re-opens the selection tool's currently-selected text element for
                // editing, in the same composing UI `tool text`'s click-to-place uses —
                // see `tools::Tool::selected_element`'s doc comment on why this needs that
                // query instead of reaching into `SelectionTool` directly.
                Key::Named(NamedKey::Enter) => {
                    if let Some(id) = self.tools.active_tool().selected_element() {
                        if let Some(element) = self.scene.element(id) {
                            if let ElementKind::Text { content, font_size } = &element.kind {
                                let bbox = element.path.bounding_box();
                                self.text_input_board_pos = DVec2::new(bbox.x0, bbox.y0);
                                self.text_input_text = content.clone();
                                self.text_input_cursor = self.text_input_text.chars().count();
                                self.text_input_font_size = *font_size as f64;
                                self.text_input_editing_id = Some(id);
                                self.text_input_active = true;
                                window.request_redraw();
                            }
                        }
                    }
                }
                Key::Character(text) if text.as_str() == ":" => {
                    self.command_line_active = true;
                    self.command_line_text.clear();
                    self.command_completion_index = None;
                    window.request_redraw();
                }
                Key::Character(text)
                    if text.len() == 1
                        && text.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
                {
                    self.trigger_macro(text.as_str());
                    window.request_redraw();
                }
                Key::Named(named) if function_key_label(*named).is_some() => {
                    // Unwrap is safe: the guard above already confirmed `Some`.
                    self.trigger_camera_macro(function_key_label(*named).unwrap());
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

    /// Applies the macro bound to `digit` (a single-character `"0"`-`"9"` string), if
    /// one's been set via `:m` — switches both the active tool and the current drawing
    /// color in one keystroke. Silently does nothing for an unset digit, same as an
    /// unbound regular keybinding already does.
    fn trigger_macro(&mut self, digit: &str) {
        let Some(macro_) = self.macros.get(digit) else {
            return;
        };
        let tool = macro_.tool.clone();
        let color = macro_.color.clone();
        if !self.tools.set_active(&tool) {
            self.status = Some((format!("macro {digit}: unknown tool {tool}"), true));
            return;
        }
        match color::parse_color(&color) {
            Some(color) => self.current_color = color,
            None => {
                self.status = Some((format!("macro {digit}: unknown color {color}"), true));
                return;
            }
        }
        self.status = Some((format!("macro {digit}: {tool} / {color}"), false));
    }

    /// Applies the camera macro bound to `key` (e.g. `"F3"`), if one's been set via
    /// `:fm` — jumps the camera's pan+zoom straight back, no tool/color change (that's
    /// `trigger_macro`'s job). Silently does nothing for an unbound key, same as
    /// `trigger_macro`.
    fn trigger_camera_macro(&mut self, key: &str) {
        let Some(camera_macro) = self.camera_macros.get(key) else {
            return;
        };
        self.camera.pan = DVec2::from_array(camera_macro.pan);
        self.camera.zoom = camera_macro.zoom;
        self.status = Some((format!("camera macro {key}: restored"), false));
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
    let board_color = config.theme.board_color;
    let registry = commands::CommandRegistry::with_builtin_commands();
    let macros = macros::load();
    let camera_macros = camera_macros::load();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App {
        config,
        registry,
        macros,
        camera_macros,
        // `#[derive(Default)]` would otherwise zero this to black — see the field's doc
        // comment.
        current_color: blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
        // `BackgroundMode` has no `Default` impl (deliberately — see its doc comment: an
        // arbitrary default color would be the wrong kind of silent), so it's set here
        // from the loaded theme, same as `current_color` above.
        background: blueboard_render::BackgroundMode::Solid(board_color),
        ..Default::default()
    };
    event_loop.run_app(&mut app).expect("event loop error");
}
