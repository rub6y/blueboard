//! The `:`-command registry. Every command is one [`Command`] implementation registered
//! by name; `config.rs`'s keybindings resolve to these same names, so there's one
//! vocabulary for both the command line and keyboard shortcuts (PLAN.md's architecture
//! section — this is the concrete mechanism meant to make adding commands later, e.g.
//! from an addon, not require touching existing command code).

use blueboard_core::file_format;
use blueboard_core::scene::Scene;
use kurbo::Shape as _;

use crate::{App, DEFAULT_BOARD_PATH};

pub trait Command {
    fn name(&self) -> &'static str;

    /// Argument hint shown by `:help` after the command name (e.g. `"[name]"`,
    /// `"<name>"`) — empty (the default) for a command that takes no arguments.
    fn usage(&self) -> &'static str {
        ""
    }

    /// Whether running this command should be recorded as an undo point (the app takes a
    /// scene snapshot around every run and hands it to `History::record` when this is
    /// `true`). Commands whose scene changes are the history mechanism itself — `undo`
    /// and `redo` — return `false`: recording them would push an entry that "undoes the
    /// undo", re-applying the very edit the user just stepped back from.
    fn records_undo(&self) -> bool {
        true
    }

    /// Run the command with everything after the command name (and separating
    /// whitespace) as `args`. `Ok` is a status message to show in the command bar; `Err`
    /// is an error message, shown in the command bar's error color.
    fn run(&self, app: &mut App, args: &str) -> Result<String, String>;

    /// Completions for this command's *own* argument string — `args` is everything typed
    /// after the command name and its separating space so far, which may itself include
    /// an already-settled sub-command token (e.g. `"style "` for `:grid style <Tab>`, or
    /// `"sty"` for `:grid sty<Tab>`). Returns `(fixed_suffix, candidates)`: `fixed_suffix`
    /// is appended right after `"<command name> "` to build the completed line's fixed
    /// part (e.g. `"style "` once the user's past that token), and each candidate's own
    /// text goes after that — the same `(fixed_prefix, candidates)` shape
    /// `main.rs`'s `command_line_completions` already uses for the top-level "which
    /// command" case, recursed one level into a specific command's own arguments.
    /// Default empty: most commands' arguments are freeform (a path, a number, raw hex)
    /// and not meaningfully completable — only enum-like arguments (grid style, a named
    /// color, background's color/gradient/image subcommand, …) implement this.
    fn complete(&self, _args: &str) -> (&'static str, Vec<(&'static str, &'static str)>) {
        ("", Vec::new())
    }
}

#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
}

impl CommandRegistry {
    pub fn with_builtin_commands() -> Self {
        let mut registry = Self::default();
        registry.register(SaveCommand);
        registry.register(OpenCommand);
        registry.register(ClearCommand);
        registry.register(NewCommand);
        registry.register(ToolCommand);
        registry.register(DeleteCommand);
        registry.register(YankCommand);
        registry.register(PasteCommand);
        registry.register(ColorCommand);
        registry.register(WidthCommand);
        registry.register(GridCommand);
        registry.register(BackgroundCommand);
        registry.register(GifCommand);
        registry.register(MacroCommand);
        registry.register(CameraMacroCommand);
        registry.register(UndoCommand);
        registry.register(RedoCommand);
        registry.register(HelpCommand);
        registry.register(QuitCommand);
        registry
    }

    pub fn register(&mut self, command: impl Command + 'static) {
        self.commands.push(Box::new(command));
    }

    /// `:help`'s output: every registered command's name and usage hint, one line. Built
    /// here, not in `HelpCommand::run`, because by the time a command's `run` is called
    /// `app`'s own `registry` field has been swapped out for the duration of the call (see
    /// `main.rs`'s `run_command`) — `self` here is the one live copy that still has the
    /// full command list to read.
    fn help_text(&self) -> String {
        self.commands
            .iter()
            .map(|command| {
                let usage = command.usage();
                if usage.is_empty() {
                    format!(":{}", command.name())
                } else {
                    format!(":{} {usage}", command.name())
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    }

    /// Whether running `name` should be recorded as an undo point — see
    /// [`Command::records_undo`]. An unknown name reports `true`, but it also fails to
    /// run at all, so the snapshot taken around it matches itself and records nothing.
    pub fn records_undo(&self, name: &str) -> bool {
        match self.commands.iter().find(|command| command.name() == name) {
            Some(command) => command.records_undo(),
            None => true,
        }
    }

    /// Commands whose name starts with `prefix`, as `(name, usage)` pairs — same data
    /// `help_text` prints, filtered. Used by the command line's Tab-completion; kept here
    /// rather than in `blueboard-render` so `CommandRegistry` doesn't need a dependency on
    /// render-crate presentation types just to answer "what matches so far."
    pub fn completions(&self, prefix: &str) -> Vec<(&'static str, &'static str)> {
        self.commands
            .iter()
            .filter(|command| command.name().starts_with(prefix))
            .map(|command| (command.name(), command.usage()))
            .collect()
    }

    /// Delegates to `name`'s own [`Command::complete`] — the command-line's "what comes
    /// after the command name" completion path, one level deeper than `completions`'s
    /// "which command" path above. Empty (no completions, and no fixed suffix) for an
    /// unknown command name.
    pub fn complete_args(
        &self,
        name: &str,
        args: &str,
    ) -> (&'static str, Vec<(&'static str, &'static str)>) {
        self.commands
            .iter()
            .find(|command| command.name() == name)
            .map(|command| command.complete(args))
            .unwrap_or(("", Vec::new()))
    }

    pub fn run(&self, app: &mut App, name: &str, args: &str) -> Result<String, String> {
        match self.commands.iter().find(|command| command.name() == name) {
            // Intercepted before reaching `HelpCommand::run` — see `help_text`'s doc
            // comment for why the listing can't be built from inside a `Command::run`.
            Some(_) if name == "help" => Ok(self.help_text()),
            Some(command) => command.run(app, args),
            None => Err(format!("unknown command: {name}")),
        }
    }
}

/// `.bbd` extension `:save`/`:open` append to a bare name — so `:save foo` and
/// `:open foo` both mean `foo.bbd`, without the user having to type the extension
/// (or getting a different file if they forget it on open vs. save).
const BOARD_EXTENSION: &str = ".bbd";

/// Resolve a `:save`/`:open` argument to an actual file path: no argument means
/// [`DEFAULT_BOARD_PATH`]; otherwise the given name, with [`BOARD_EXTENSION`] appended
/// unless it's already there (so `:save board.bbd` doesn't become `board.bbd.bbd`).
fn resolve_board_path(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return DEFAULT_BOARD_PATH.to_string();
    }
    if trimmed.to_ascii_lowercase().ends_with(BOARD_EXTENSION) {
        trimmed.to_string()
    } else {
        format!("{trimmed}{BOARD_EXTENSION}")
    }
}

struct SaveCommand;
impl Command for SaveCommand {
    fn name(&self) -> &'static str {
        "save"
    }

    fn usage(&self) -> &'static str {
        "[name]"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let path = resolve_board_path(args);
        file_format::save(&app.scene, &path)
            .map(|()| format!("saved {} element(s) to {path}", app.scene.len()))
            .map_err(|err| format!("save to {path} failed: {err}"))
    }
}

struct OpenCommand;
impl Command for OpenCommand {
    fn name(&self) -> &'static str {
        "open"
    }

    fn usage(&self) -> &'static str {
        "[name]"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let path = resolve_board_path(args);
        match file_format::load(&path) {
            Ok(scene) => {
                let count = scene.len();
                app.scene = scene;
                Ok(format!("loaded {count} element(s) from {path}"))
            }
            Err(err) => Err(format!("open {path} failed: {err}")),
        }
    }
}

struct ClearCommand;
impl Command for ClearCommand {
    fn name(&self) -> &'static str {
        "clear"
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        let count = app.scene.len();
        app.scene = Scene::new();
        Ok(format!("cleared {count} element(s)"))
    }
}

/// Same effect as `clear` — there's no concept of multiple distinct boards/files yet to
/// make `:new` meaningfully different. Kept as its own command (not just an alias
/// dispatched earlier) so it shows up in the registry under its own name.
struct NewCommand;
impl Command for NewCommand {
    fn name(&self) -> &'static str {
        "new"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        ClearCommand.run(app, args)
    }
}

struct ToolCommand;
impl Command for ToolCommand {
    fn name(&self) -> &'static str {
        "tool"
    }

    fn usage(&self) -> &'static str {
        "<name>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let name = args.trim();
        if app.tools.set_active(name) {
            Ok(format!("tool: {name}"))
        } else {
            Err(format!("unknown tool: {name}"))
        }
    }
}

/// Deletes whatever the active tool currently has selected — only the selection-based
/// tools (`select`, `select-rect`, `select-lasso`) have anything to delete; every other
/// tool's `Tool::delete_selection` default no-ops, which is reported back as an error
/// here rather than silently succeeding.
struct DeleteCommand;
impl Command for DeleteCommand {
    fn name(&self) -> &'static str {
        "delete"
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        if app.tools.active_tool_mut().delete_selection(&mut app.scene) {
            Ok("deleted selection".to_string())
        } else {
            Err("nothing selected".to_string())
        }
    }
}

/// Copies whatever's currently selected (via whichever selection tool is active — `select`,
/// `select-rect`, or `select-lasso`, see [`crate::tools::Tool::selected_elements`]) into
/// `app.clipboard`, replacing its previous contents. Bound to `y` by default, vim's yank.
/// Doesn't touch the scene, so there's nothing for undo to record here — `PasteCommand` is
/// the half of this pair that does.
struct YankCommand;
impl Command for YankCommand {
    fn name(&self) -> &'static str {
        "yank"
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        let ids = app.tools.active_tool().selected_elements();
        if ids.is_empty() {
            return Err("nothing selected".to_string());
        }
        app.clipboard = ids
            .iter()
            .filter_map(|id| app.scene.element(*id))
            .map(|element| (element.path.clone(), element.color, element.kind.clone()))
            .collect();
        Ok(format!("yanked {} element(s)", app.clipboard.len()))
    }
}

/// Pastes `app.clipboard`'s contents as fresh elements (new ids, originals untouched),
/// recentered as a group — relative positions between pasted elements are preserved, but
/// the whole group's combined bounding box is shifted so its center lands at the cursor's
/// current board position. Bound to `Y` by default: vim's own paste key, lowercase `p`, is
/// already this app's pen-tool keybinding, so paste sits on yank's shift-variant instead —
/// same relationship `u`/`U` (undo/redo) already has.
struct PasteCommand;
impl Command for PasteCommand {
    fn name(&self) -> &'static str {
        "paste"
    }

    /// `app.clipboard` (`:yank`'d board elements) wins if it's non-empty; otherwise falls
    /// back to whatever image is on the *OS* clipboard (e.g. copied from a browser, a
    /// screenshot tool) — one `Y` covers both "paste what I copied on the board" and
    /// "paste what I copied outside it," rather than needing a second keybinding for the
    /// external-image case.
    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        let cursor_board = cursor_board_position(app)?;
        if app.clipboard.is_empty() {
            return paste_external_image(app, cursor_board);
        }

        let mut combined: Option<kurbo::Rect> = None;
        for (path, _, _) in &app.clipboard {
            let bbox = path.bounding_box();
            combined = Some(match combined {
                Some(existing) => existing.union(bbox),
                None => bbox,
            });
        }
        // Unreachable (the emptiness check above already returned), but keeps this free
        // of an `.unwrap()`.
        let Some(combined) = combined else {
            return Err("clipboard is empty".to_string());
        };
        let center = combined.center();
        let delta = glam::DVec2::new(cursor_board.x - center.x, cursor_board.y - center.y);
        let affine = kurbo::Affine::translate((delta.x, delta.y));

        let clipboard = app.clipboard.clone();
        let pasted_ids: Vec<u64> = clipboard
            .into_iter()
            .map(|(path, color, kind)| app.scene.add_element_with_kind(affine * path, color, kind))
            .collect();
        select_pasted(app, &pasted_ids);
        Ok(format!("pasted {} element(s)", pasted_ids.len()))
    }
}

/// Switches to the `select` tool (if some other tool was active — e.g. `pen`, which was
/// likely the active tool right before a paste) and selects `ids` on it, so a paste is
/// immediately visible as selected (handles showing) and draggable, regardless of what
/// tool was active beforehand. `select_elements` alone isn't enough: it only has an effect
/// on whichever tool happens to already be active, and most tools (anything but
/// `select`/`select-rect`/`select-lasso`) just no-op it.
fn select_pasted(app: &mut App, ids: &[u64]) {
    let active_name = app.tools.active_tool().name();
    if !matches!(active_name, "select" | "select-rect" | "select-lasso") {
        app.tools.set_active("select");
    }
    app.tools.active_tool_mut().select_elements(&app.scene, ids);
}

/// Places an animated GIF: decodes every frame up front, commits a plain `ElementKind::
/// Image` holding the first frame (so it's selectable/resizable/saved exactly like a
/// static pasted image, no core/render changes needed for any of that), and registers it
/// in `app.gif_players` for `crate::gif::advance_players` to cycle frames on — see that
/// module's doc comment for why animation state lives entirely in `blueboard-app` instead.
struct GifCommand;
impl Command for GifCommand {
    fn name(&self) -> &'static str {
        "gif"
    }

    fn usage(&self) -> &'static str {
        "<path>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let path = args.trim();
        if path.is_empty() {
            return Err("usage: gif <path>".to_string());
        }
        let frames = crate::gif::decode(path).map_err(|err| format!("{path}: {err}"))?;
        let Some(first) = frames.first() else {
            return Err(format!("{path}: no frames"));
        };
        let cursor_board = cursor_board_position(app)?;
        let id = app.scene.add_image_element(
            cursor_board,
            blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
            first.image.clone(),
        );
        let Some(renderer) = app.renderer.as_mut() else {
            return Err("renderer not ready yet".to_string());
        };
        renderer.set_element_texture(id, &first.image.rgba, first.image.width, first.image.height);
        let frame_count = frames.len();
        app.gif_players
            .insert(id, crate::gif::GifPlayer::new(frames));
        select_pasted(app, &[id]);
        Ok(format!("playing {path} ({frame_count} frames)"))
    }
}

fn cursor_board_position(app: &App) -> Result<glam::DVec2, String> {
    let Some(renderer) = app.renderer.as_ref() else {
        return Err("renderer not ready yet".to_string());
    };
    let (width, height) = renderer.viewport_size();
    let viewport = glam::DVec2::new(width as f64, height as f64);
    Ok(app.camera.screen_to_board(app.last_cursor, viewport))
}

/// Reads an image off the *OS* clipboard (via `arboard`, already RGBA8 — no decode step
/// needed, unlike `:background image <path>`'s file-based `image`-crate decode), pastes
/// it centered at `cursor_board`, uploads its texture immediately (unlike `latex`'s
/// deferred-to-next-redraw compile, this is cheap and synchronous — no process to spawn),
/// and selects it, same as a board-object paste.
fn paste_external_image(app: &mut App, cursor_board: glam::DVec2) -> Result<String, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("clipboard unavailable: {err}"))?;
    let image = clipboard.get_image().map_err(|err| {
        log::warn!("arboard clipboard image read failed: {err}");
        format!("clipboard has no image: {err}")
    })?;
    let (width, height) = (image.width as u32, image.height as u32);
    let image_data = blueboard_core::scene::ImageData {
        rgba: image.bytes.into_owned(),
        width,
        height,
    };

    let id = app.scene.add_image_element(
        cursor_board,
        blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
        image_data.clone(),
    );
    let Some(renderer) = app.renderer.as_mut() else {
        return Err("renderer not ready yet".to_string());
    };
    renderer.set_element_texture(id, &image_data.rgba, width, height);
    select_pasted(app, &[id]);
    Ok(format!("pasted image ({width}x{height})"))
}

/// Sets the color the pen and shape tools draw their next element in, or toggles the
/// interactive picker. Named colors and `#rrggbb`/`rrggbb` hex both go through
/// `crate::color::parse_color`; `picker` is handled separately since it's not a color at
/// all.
struct ColorCommand;
impl Command for ColorCommand {
    fn name(&self) -> &'static str {
        "color"
    }

    fn usage(&self) -> &'static str {
        "<name|#hex|picker>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let arg = args.trim();
        if arg.eq_ignore_ascii_case("picker") {
            app.color_picker_open = !app.color_picker_open;
            let state = if app.color_picker_open {
                "open"
            } else {
                "closed"
            };
            return Ok(format!("color picker: {state}"));
        }
        match crate::color::parse_color(arg) {
            Some(color) => {
                app.current_color = color;
                Ok(format!("color: {arg}"))
            }
            None => Err(format!("unknown color: {arg}")),
        }
    }

    fn complete(&self, args: &str) -> (&'static str, Vec<(&'static str, &'static str)>) {
        // Unifies the named-color list and the interactive picker into one completable
        // list — arrow/Tab onto "picker" and Enter opens it, same as typing it by hand.
        let mut candidates = crate::color::name_completions(args);
        if "picker".starts_with(args) {
            candidates.push(("picker", ""));
        }
        ("", candidates)
    }
}

/// Board-space bounds `:width` clamps to — prevents a degenerate zero/negative width (would
/// tessellate to nothing) or an absurdly thick one. Mirrors `GridCommand`'s own
/// `GRID_MIN_SPACING`/`GRID_MAX_SPACING` pair in shape and purpose.
const WIDTH_MIN: f32 = 0.5;
const WIDTH_MAX: f32 = 50.0;

/// Sets the stroke width the pen and shape tools draw their next element with —
/// `blueboard_core::scene::ElementKind::Stroke`'s own `width` field, board-space (scales
/// with zoom like the path itself). Remembered per-element once drawn, same "current_*
/// setting, baked in at commit time" shape `:color` already has.
struct WidthCommand;
impl Command for WidthCommand {
    fn name(&self) -> &'static str {
        "width"
    }

    fn usage(&self) -> &'static str {
        "<number>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let width: f32 = args
            .trim()
            .parse()
            .map_err(|_| format!("width must be a number, not {:?}", args.trim()))?;
        if width <= 0.0 {
            return Err("width must be positive".to_string());
        }
        app.current_stroke_width = width.clamp(WIDTH_MIN, WIDTH_MAX);
        Ok(format!("width: {}", app.current_stroke_width))
    }
}

/// Multiplicative step `:grid bigger`/`:grid smaller` scale the spacing by each press.
const GRID_STEP_FACTOR: f32 = 1.5;
/// Spacing bounds (board-space), both for the step commands and a direct `:grid <number>`
/// — prevents a degenerate near-zero or absurdly coarse grid.
const GRID_MIN_SPACING: f32 = 5.0;
const GRID_MAX_SPACING: f32 = 5000.0;

/// Shows/hides the reference grid, or changes its spacing — relative (`bigger`/`smaller`,
/// multiplying by [`GRID_STEP_FACTOR`]) or an exact board-space number.
struct GridCommand;
impl Command for GridCommand {
    fn name(&self) -> &'static str {
        "grid"
    }

    fn usage(&self) -> &'static str {
        "<show|hide|bigger|smaller|size|style>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        match args.trim() {
            "show" => {
                app.grid.visible = true;
                Ok("grid: shown".to_string())
            }
            "hide" => {
                app.grid.visible = false;
                Ok("grid: hidden".to_string())
            }
            "bigger" => {
                app.grid.spacing =
                    (app.grid.spacing * GRID_STEP_FACTOR).clamp(GRID_MIN_SPACING, GRID_MAX_SPACING);
                Ok(format!("grid spacing: {}", app.grid.spacing))
            }
            "smaller" => {
                app.grid.spacing =
                    (app.grid.spacing / GRID_STEP_FACTOR).clamp(GRID_MIN_SPACING, GRID_MAX_SPACING);
                Ok(format!("grid spacing: {}", app.grid.spacing))
            }
            arg if arg.starts_with("style") => {
                let style_name = arg.strip_prefix("style").unwrap_or("").trim();
                app.grid.style = match style_name {
                    "lines" => blueboard_render::GridStyle::Lines,
                    "horizontal" => blueboard_render::GridStyle::HorizontalOnly,
                    "hex" => blueboard_render::GridStyle::Hex,
                    "dots" => blueboard_render::GridStyle::Dots,
                    other => {
                        return Err(format!(
                            "unknown grid style {other:?} (lines|horizontal|hex|dots)"
                        ));
                    }
                };
                Ok(format!("grid style: {style_name}"))
            }
            arg => match arg.parse::<f32>() {
                Ok(spacing) if spacing > 0.0 => {
                    app.grid.spacing = spacing.clamp(GRID_MIN_SPACING, GRID_MAX_SPACING);
                    Ok(format!("grid spacing: {}", app.grid.spacing))
                }
                _ => Err(format!("unknown grid command: {arg}")),
            },
        }
    }

    fn complete(&self, args: &str) -> (&'static str, Vec<(&'static str, &'static str)>) {
        const TOP_LEVEL: &[&str] = &["show", "hide", "bigger", "smaller", "style"];
        const STYLES: &[&str] = &["lines", "horizontal", "hex", "dots"];
        if let Some(style_arg) = args.strip_prefix("style ") {
            return (
                "style ",
                STYLES
                    .iter()
                    .filter(|s| s.starts_with(style_arg))
                    .map(|&s| (s, ""))
                    .collect(),
            );
        }
        (
            "",
            TOP_LEVEL
                .iter()
                .filter(|s| s.starts_with(args))
                .map(|&s| (s, ""))
                .collect(),
        )
    }
}

/// Sets the board's background fill — a solid color, a two-color gradient, or an image
/// file (PNG/JPEG/etc, decoded via the `image` crate) with a chosen fit mode. Subcommand
/// style mirrors `:grid`. Solid/gradient settings live entirely in `app.background`
/// (cheap to recompute every frame — see `BackgroundMode`'s doc comment); an image is
/// decoded and uploaded to the GPU once, here, via `Renderer::set_background_image`, since
/// redoing that every frame would be wasteful.
struct BackgroundCommand;
impl Command for BackgroundCommand {
    fn name(&self) -> &'static str {
        "background"
    }

    fn usage(&self) -> &'static str {
        "<color <hex>|gradient <hex> <hex> <deg>|image <path> [zoom|tile]>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let mut parts = args.trim().splitn(2, ' ');
        let subcommand = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();
        match subcommand {
            "color" => {
                let color = crate::color::parse_color(rest)
                    .ok_or_else(|| format!("unknown color {rest:?}"))?;
                app.background = blueboard_render::BackgroundMode::Solid(color);
                Ok(format!(
                    "background: solid {}",
                    crate::color::color_to_hex(color)
                ))
            }
            "gradient" => {
                let mut words = rest.split_whitespace();
                let usage = "usage: background gradient <hex> <hex> <deg>";
                let from = words.next().ok_or(usage)?;
                let to = words.next().ok_or(usage)?;
                let angle_deg: f32 = words
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .map_err(|_| "angle must be a number of degrees".to_string())?;
                let from = crate::color::parse_color(from)
                    .ok_or_else(|| format!("unknown color {from:?}"))?;
                let to =
                    crate::color::parse_color(to).ok_or_else(|| format!("unknown color {to:?}"))?;
                app.background = blueboard_render::BackgroundMode::Gradient {
                    from,
                    to,
                    angle_deg,
                };
                Ok(format!("background: gradient, {angle_deg} deg"))
            }
            "image" => {
                let mut words = rest.split_whitespace();
                let path = words
                    .next()
                    .ok_or("usage: background image <path> [zoom|tile]")?;
                let fit = match words.next().unwrap_or("zoom") {
                    "zoom" => blueboard_render::ImageFit::ZoomWithCamera,
                    "tile" => blueboard_render::ImageFit::Tile,
                    other => return Err(format!("unknown fit {other:?} (zoom|tile)")),
                };
                let decoded =
                    image::open(path).map_err(|err| format!("failed to open {path}: {err}"))?;
                let rgba = decoded.to_rgba8();
                let (width, height) = rgba.dimensions();
                let renderer = app
                    .renderer
                    .as_mut()
                    .ok_or("renderer not ready yet".to_string())?;
                renderer.set_background_image(rgba.as_raw(), width, height);
                app.background = blueboard_render::BackgroundMode::Image(fit);
                Ok(format!("background: image {path} ({width}x{height})"))
            }
            other => Err(format!(
                "unknown background command: {other:?} (color|gradient|image)"
            )),
        }
    }

    fn complete(&self, args: &str) -> (&'static str, Vec<(&'static str, &'static str)>) {
        const TOP_LEVEL: &[&str] = &["color", "gradient", "image"];
        if let Some(rest) = args.strip_prefix("color ") {
            return ("color ", crate::color::name_completions(rest));
        }
        // Only the gradient's *first* hex color is completable as a named color —
        // the second color, angle, and the image subcommand's path/fit args are all
        // freeform, same "enum-like only" scope `Command::complete`'s doc comment sets.
        if let Some(rest) = args.strip_prefix("gradient ") {
            if !rest.contains(' ') {
                return ("gradient ", crate::color::name_completions(rest));
            }
            return ("", Vec::new());
        }
        (
            "",
            TOP_LEVEL
                .iter()
                .filter(|s| s.starts_with(args))
                .map(|&s| (s, ""))
                .collect(),
        )
    }
}

/// Binds the active tool and current drawing color to a number key — pressing that digit
/// afterward (outside the command line) switches both at once. Persisted to
/// `macros.toml` (`crate::macros`) right when set, so it survives a restart.
struct MacroCommand;
impl Command for MacroCommand {
    fn name(&self) -> &'static str {
        "m"
    }

    fn usage(&self) -> &'static str {
        "<0-9>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let digit = args.trim();
        if digit.len() != 1 || !digit.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Err(format!(
                "macro key must be a single digit 0-9, not {digit:?}"
            ));
        }
        let tool = app.tools.active_tool().name().to_string();
        let color = crate::color::color_to_hex(app.current_color);
        app.macros.insert(
            digit.to_string(),
            crate::macros::Macro {
                tool: tool.clone(),
                color: color.clone(),
            },
        );
        crate::macros::save(&app.macros);
        Ok(format!("macro {digit}: {tool} / {color}"))
    }
}

/// Binds the camera's current pan+zoom to a function key (`F1`-`F12`) — pressing that
/// key afterward (outside the command line) jumps the camera straight back, no tool/color
/// change (that's `:m`'s job). Persisted to `camera_macros.toml`
/// (`crate::camera_macros`) right when set, so it survives a restart.
struct CameraMacroCommand;
impl Command for CameraMacroCommand {
    fn name(&self) -> &'static str {
        "fm"
    }

    fn usage(&self) -> &'static str {
        "<1-12>"
    }

    fn run(&self, app: &mut App, args: &str) -> Result<String, String> {
        let number: u32 = args
            .trim()
            .parse()
            .map_err(|_| format!("camera macro key must be 1-12, not {:?}", args.trim()))?;
        if !(1..=12).contains(&number) {
            return Err(format!("camera macro key must be 1-12, not {number}"));
        }
        let key = format!("F{number}");
        app.camera_macros.insert(
            key.clone(),
            crate::camera_macros::CameraMacro {
                pan: app.camera.pan.to_array(),
                zoom: app.camera.zoom,
            },
        );
        crate::camera_macros::save(&app.camera_macros);
        Ok(format!("camera macro {key}: saved"))
    }
}

struct UndoCommand;
impl Command for UndoCommand {
    fn name(&self) -> &'static str {
        "undo"
    }

    fn records_undo(&self) -> bool {
        false
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        if app.history.undo(&mut app.scene) {
            Ok("undone".to_string())
        } else {
            Err("nothing to undo".to_string())
        }
    }
}

struct RedoCommand;
impl Command for RedoCommand {
    fn name(&self) -> &'static str {
        "redo"
    }

    fn records_undo(&self) -> bool {
        false
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        if app.history.redo(&mut app.scene) {
            Ok("redone".to_string())
        } else {
            Err("nothing to redo".to_string())
        }
    }
}

/// Lists every registered command. Registered like any other command so `:help` is
/// discoverable the same way the rest are (present in the registry, listed in its own
/// output) — but `run` here is never actually called: `CommandRegistry::run` intercepts
/// `"help"` before dispatch, since building the listing needs the live registry (see
/// `CommandRegistry::help_text`), and this method's `&mut App` doesn't have one — `app`'s
/// `registry` field is swapped out for the duration of any command's `run` call. Returns
/// an `Err` rather than panicking if that interception is ever bypassed, in keeping with
/// this project's crash-resilience stance (see the ledger's Phase 4 entry).
struct HelpCommand;
impl Command for HelpCommand {
    fn name(&self) -> &'static str {
        "help"
    }

    fn records_undo(&self) -> bool {
        false
    }

    fn run(&self, _app: &mut App, _args: &str) -> Result<String, String> {
        Err("help text is built by CommandRegistry::run directly".to_string())
    }
}

struct QuitCommand;
impl Command for QuitCommand {
    fn name(&self) -> &'static str {
        "q"
    }

    fn run(&self, app: &mut App, _args: &str) -> Result<String, String> {
        app.should_quit = true;
        Ok(String::new())
    }
}
